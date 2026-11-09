use std::{collections::HashMap, mem::size_of, path::Path, sync::Mutex};
use windows_sys::{
    core::BOOL,
    Win32::{
        Foundation::*,
        Graphics::Gdi::ClientToScreen,
        System::Diagnostics::Debug::ReadProcessMemory,
        System::Threading::*,
        UI::{Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*},
    },
};

struct Attached {
    hwnd: isize,
    pid: u32,
    parent: isize,
    owner: isize,
    style: isize,
    placement: WINDOWPLACEMENT,
    rect: RECT,
    browser: isize,
    browser_style: isize,
    header_height: i32,
    layout: Option<[i32; 4]>,
}
#[derive(Default)]
pub(super) struct Client(Mutex<Vec<Attached>>);

fn failure(action: &str) -> String {
    format!("{action}: {}", std::io::Error::last_os_error())
}

unsafe fn remote_bytes(process: HANDLE, address: usize, length: usize) -> Option<Vec<u8>> {
    if address < 0x10000 || length > 16384 {
        return None;
    }
    let mut bytes = vec![0; length];
    let mut read = 0;
    (ReadProcessMemory(
        process,
        address as *const _,
        bytes.as_mut_ptr() as *mut _,
        length,
        &mut read,
    ) != 0
        && read == length)
        .then_some(bytes)
}

unsafe fn remote_pointer(process: HANDLE, address: usize) -> Option<usize> {
    Some(usize::from_le_bytes(
        remote_bytes(process, address, size_of::<usize>())?
            .try_into()
            .ok()?,
    ))
}

unsafe fn remote_class(process: HANDLE, object: usize) -> Option<(String, usize)> {
    let vmt = remote_pointer(process, object)?;
    // shortcut: Delphi x64 VMT offsets verified on FME 3.7; revalidate for another compiler build.
    let name = remote_pointer(process, vmt.checked_sub(136)?)?;
    let length = remote_bytes(process, name, 1)?[0] as usize;
    if !(2..=80).contains(&length) {
        return None;
    }
    let class = String::from_utf8(remote_bytes(process, name + 1, length)?).ok()?;
    if !class.starts_with('T')
        || !class
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || v == b'_')
    {
        return None;
    }
    let size = u32::from_le_bytes(
        remote_bytes(process, vmt.checked_sub(128)?, 4)?
            .try_into()
            .ok()?,
    ) as usize;
    (size <= 16384 && size >= 8).then_some((class, size))
}

unsafe fn remote_field(process: HANDLE, object: usize, expected: &str) -> Option<usize> {
    let (_, size) = remote_class(process, object)?;
    let bytes = remote_bytes(process, object, size)?;
    for field in bytes.chunks_exact(size_of::<usize>()).skip(1) {
        let pointer = usize::from_le_bytes(field.try_into().ok()?);
        if let Some((class, _)) = remote_class(process, pointer) {
            if class == expected {
                return Some(pointer);
            }
        }
    }
    None
}

unsafe extern "system" fn control_property(
    _: HWND,
    name: windows_sys::core::PCWSTR,
    value: HANDLE,
    data: usize,
) -> BOOL {
    if name as usize >= 0x10000 {
        if [68, 101, 108, 112, 104, 105]
            .iter()
            .enumerate()
            .all(|(index, unit)| *name.add(index) == *unit)
        {
            *(data as *mut usize) = value as usize;
            return 0;
        }
    }
    1
}

unsafe fn delphi_menu(hwnd: HWND, pid: u32) -> HMENU {
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
    if process.is_null() {
        return std::ptr::null_mut();
    }
    let mut control = 0usize;
    EnumPropsExW(
        hwnd,
        Some(control_property),
        &mut control as *mut _ as isize,
    );
    let menu = (|| {
        if remote_class(process, control)?.0 != "TMainFrame" {
            return None;
        }
        let object = remote_field(process, control, "TMainMenu")?;
        let root = remote_field(process, object, "TMenuItem")?;
        // shortcut: TMenuItem.FHandle is at +352 in this verified FME build; IsMenu rejects mismatches.
        let handle = remote_pointer(process, root.checked_add(352)?)? as HMENU;
        (IsMenu(handle) != 0 && GetMenuItemCount(handle) > 0).then_some(handle)
    })()
    .unwrap_or(std::ptr::null_mut());
    CloseHandle(process);
    menu
}

#[derive(Default)]
struct MenuLabels {
    commands: HashMap<u32, Vec<u16>>,
    popups: HashMap<usize, Vec<u16>>,
}

unsafe fn client_menu(hwnd: HWND, pid: u32) -> HMENU {
    // FME recreates its menus on frame changes; never retain or restore a foreign HMENU.
    let menu = GetMenu(hwnd);
    if menu.is_null() {
        delphi_menu(hwnd, pid)
    } else {
        menu
    }
}

unsafe fn collect_menu_labels(
    process: HANDLE,
    item: usize,
    depth: usize,
    remaining: &mut usize,
    labels: &mut MenuLabels,
) -> Option<()> {
    if depth > 8 || *remaining == 0 || remote_class(process, item)?.0 != "TMenuItem" {
        return None;
    }
    *remaining -= 1;
    // shortcut: Caption/Command/Items/Handle offsets verified by REA on this Delphi x64 build.
    if let Some(text) = remote_pointer(process, item + 120) {
        if let Some(length) = text
            .checked_sub(4)
            .and_then(|p| remote_bytes(process, p, 4))
        {
            let length = u32::from_le_bytes(length.try_into().ok()?) as usize;
            if (1..=1024).contains(&length) {
                if let Some(bytes) = remote_bytes(process, text, length * 2) {
                    let mut caption: Vec<u16> = bytes
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect();
                    caption.push(0);
                    let command =
                        u16::from_le_bytes(remote_bytes(process, item + 176, 2)?.try_into().ok()?)
                            as u32;
                    labels.commands.insert(command, caption.clone());
                    let menu = remote_pointer(process, item + 352)?;
                    if IsMenu(menu as HMENU) != 0 {
                        labels.popups.insert(menu, caption);
                    }
                }
            }
        }
    }
    let list = remote_pointer(process, item + 192)?;
    if list == 0 {
        return Some(());
    }
    if remote_class(process, list)?.0 != "TList" {
        return None;
    }
    let count = u32::from_le_bytes(remote_bytes(process, list + 16, 4)?.try_into().ok()?) as usize;
    if count > 500 {
        return None;
    }
    let array = remote_pointer(process, list + 8)?;
    if count == 0 {
        return Some(());
    }
    for pointer in
        remote_bytes(process, array, count * size_of::<usize>())?.chunks_exact(size_of::<usize>())
    {
        collect_menu_labels(
            process,
            usize::from_le_bytes(pointer.try_into().ok()?),
            depth + 1,
            remaining,
            labels,
        );
    }
    Some(())
}

unsafe fn delphi_menu_labels(hwnd: HWND, pid: u32) -> MenuLabels {
    let mut labels = MenuLabels::default();
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
    if process.is_null() {
        return labels;
    }
    let mut control = 0usize;
    EnumPropsExW(
        hwnd,
        Some(control_property),
        &mut control as *mut _ as isize,
    );
    let root = (|| {
        if remote_class(process, control)?.0 != "TMainFrame" {
            return None;
        }
        remote_field(
            process,
            remote_field(process, control, "TMainMenu")?,
            "TMenuItem",
        )
    })();
    if let Some(root) = root {
        collect_menu_labels(process, root, 0, &mut 500, &mut labels);
    }
    CloseHandle(process);
    labels
}

unsafe fn fme_pid(hwnd: HWND) -> Option<u32> {
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if process.is_null() {
        return None;
    }
    let mut path = [0u16; 32768];
    let mut length = path.len() as u32;
    let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
    CloseHandle(process);
    let path = String::from_utf16_lossy(&path[..length.min(path.len() as u32) as usize]);
    (ok != 0
        && Path::new(&path)
            .file_name()?
            .to_str()?
            .eq_ignore_ascii_case("FME.exe"))
    .then_some(pid)
}

unsafe extern "system" fn enumerate(hwnd: HWND, data: LPARAM) -> BOOL {
    let mut class = [0u16; 128];
    let count = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    let class_name = String::from_utf16_lossy(&class[..count as usize]);
    if class_name == "TApplication" {
        return 1;
    }
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    if let Some(pid) = fme_pid(hwnd) {
        let mut title = [0u16; 1024];
        let count = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        if count > 0 {
            let list = &mut *(data as *mut Vec<serde_json::Value>);
            list.push(serde_json::json!({"hwnd": hwnd as usize, "pid": pid,
                "title": String::from_utf16_lossy(&title[..count as usize]), "class": class_name}));
        }
    }
    1
}

#[tauri::command]
fn fme_windows() -> Result<Vec<serde_json::Value>, String> {
    let mut list = Vec::new();
    unsafe {
        if EnumWindows(Some(enumerate), &mut list as *mut _ as LPARAM) == 0 {
            return Err(failure("Не удалось перечислить окна"));
        }
    }
    Ok(list)
}

unsafe extern "system" fn restore_application(hwnd: HWND, count: LPARAM) -> BOOL {
    let mut class = [0u16; 128];
    let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    if String::from_utf16_lossy(&class[..length as usize]) == "TApplication"
        && fme_pid(hwnd).is_some()
    {
        ShowWindowAsync(hwnd, SW_RESTORE);
        *(count as *mut u32) += 1;
    }
    1
}

#[tauri::command]
fn fme_show() -> Result<(), String> {
    let mut count = 0u32;
    unsafe {
        EnumWindows(Some(restore_application), &mut count as *mut _ as LPARAM);
    }
    if count == 0 {
        return Err("Запущенный клиент FME не найден".into());
    }
    Ok(())
}

unsafe fn set_style(hwnd: HWND, style: isize) -> Result<(), String> {
    SetLastError(0);
    if SetWindowLongPtrW(hwnd, GWL_STYLE, style) == 0 && GetLastError() != 0 {
        return Err(failure("Не удалось изменить стиль окна"));
    }
    Ok(())
}

unsafe fn set_parent(hwnd: HWND, parent: HWND) -> Result<(), String> {
    SetLastError(0);
    if SetParent(hwnd, parent).is_null() && GetLastError() != 0 {
        return Err(failure("Не удалось переместить окно"));
    }
    Ok(())
}

impl Client {
    pub(super) fn detach(&self) -> Result<(), String> {
        let mut guard = self.0.lock().map_err(|_| "Состояние окна недоступно")?;
        while let Some(saved) = guard.last() {
            unsafe {
                let hwnd = saved.hwnd as HWND;
                if IsWindow(saved.browser as HWND) != 0 {
                    set_style(saved.browser as HWND, saved.browser_style)?;
                }
                if IsWindow(hwnd) != 0 && fme_pid(hwnd) == Some(saved.pid) {
                    let visible = GetWindowLongPtrW(hwnd, GWL_STYLE) & WS_VISIBLE as isize;
                    set_parent(hwnd, saved.parent as HWND)?;
                    set_style(hwnd, (saved.style & !(WS_VISIBLE as isize)) | visible)?;
                    SetLastError(0);
                    if SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, saved.owner) == 0
                        && GetLastError() != 0
                    {
                        return Err(failure("Не удалось восстановить владельца окна"));
                    }
                    if SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    ) == 0
                    {
                        return Err(failure("Не удалось восстановить рамку"));
                    }
                    let mut placement = saved.placement;
                    if visible == 0 {
                        placement.showCmd = SW_HIDE as u32;
                    }
                    if SetWindowPlacement(hwnd, &placement) == 0 {
                        return Err(failure("Не удалось восстановить положение"));
                    }
                    if saved.placement.showCmd != SW_SHOWMINIMIZED as u32
                        && SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            saved.rect.left,
                            saved.rect.top,
                            saved.rect.right - saved.rect.left,
                            saved.rect.bottom - saved.rect.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        ) == 0
                    {
                        return Err(failure("Не удалось восстановить размеры окна"));
                    }
                }
            }
            guard.pop();
        }
        Ok(())
    }
}

#[tauri::command]
fn fme_detach(state: tauri::State<'_, Client>) -> Result<(), String> {
    state.detach()
}

#[tauri::command]
fn fme_connection(state: tauri::State<'_, Client>) -> Result<Option<serde_json::Value>, String> {
    let guard = state.0.lock().map_err(|_| "Window state is unavailable")?;
    Ok(guard
        .iter()
        .find(|item| {
            item.header_height == 0
                && unsafe {
                    IsWindow(item.hwnd as HWND) != 0 && fme_pid(item.hwnd as HWND) == Some(item.pid)
                }
        })
        .map(|item| serde_json::json!({"hwnd":item.hwnd,"pid":item.pid,"title":"FME","class":""})))
}

fn is_tab_form(class: &str, style: u32) -> bool {
    class == "TMainFrame"
        || (class != "TApplication"
            && class.starts_with('T')
            && style & (WS_CHILD | WS_POPUP) == 0
            && style & WS_MAXIMIZEBOX != 0
            && style & WS_CAPTION == WS_CAPTION)
}

unsafe extern "system" fn enumerate_forms(hwnd: HWND, data: LPARAM) -> BOOL {
    let (pid, forms) = &mut *(data as *mut (u32, Vec<(usize, bool)>));
    let mut actual = 0;
    GetWindowThreadProcessId(hwnd, &mut actual);
    if actual != *pid || GetWindowTextLengthW(hwnd) == 0 {
        return 1;
    }
    let mut class = [0u16; 128];
    let length = GetClassNameW(hwnd, class.as_mut_ptr(), 128);
    let class = String::from_utf16_lossy(&class[..length as usize]);
    if is_tab_form(&class, GetWindowLongPtrW(hwnd, GWL_STYLE) as u32) {
        forms.push((hwnd as usize, class == "TMainFrame"));
    }
    1
}

fn process_forms(pid: u32) -> Result<Vec<(usize, bool)>, String> {
    let mut context: (u32, Vec<(usize, bool)>) = (pid, Vec::new());
    unsafe {
        if EnumWindows(Some(enumerate_forms), &mut context as *mut _ as LPARAM) == 0 {
            return Err(failure("Could not enumerate FME forms"));
        }
    }
    context.1.sort_by_key(|(_, header)| !*header);
    Ok(context.1)
}

fn dialog_position(rect: [i32; 4], area: [i32; 4]) -> Option<(i32, i32)> {
    let [left, top, right, bottom] = rect;
    let [x, y, w, h] = area;
    let (width, height) = (right - left, bottom - top);
    if width <= 0 || height <= 0 || width > w || height > h {
        return None;
    }
    if left >= x && top >= y && right <= x + w && bottom <= y + h {
        return None;
    }
    Some((x + (w - width) / 2, y + (h - height) / 2))
}

unsafe extern "system" fn contain_dialog(hwnd: HWND, data: LPARAM) -> BOOL {
    let (pid, area) = &*(data as *const (u32, [i32; 4]));
    let mut actual = 0;
    GetWindowThreadProcessId(hwnd, &mut actual);
    if actual != *pid || IsWindowVisible(hwnd) == 0 || GetWindow(hwnd, GW_OWNER).is_null() {
        return 1;
    }
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    if style & WS_CAPTION != WS_CAPTION || GetWindowTextLengthW(hwnd) == 0 {
        return 1;
    }
    let mut class = [0u16; 128];
    let length = GetClassNameW(hwnd, class.as_mut_ptr(), 128);
    let class = String::from_utf16_lossy(&class[..length as usize]);
    if class == "TApplication" || is_tab_form(&class, style) {
        return 1;
    }
    let mut rect: RECT = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut rect) != 0 {
        if let Some((x, y)) = dialog_position([rect.left, rect.top, rect.right, rect.bottom], *area)
        {
            // Keep popup modality and ownership; only place a fitting dialog inside the client area.
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    1
}

unsafe fn attach_window(
    window: &tauri::WebviewWindow,
    guard: &mut Vec<Attached>,
    handle: usize,
    pid: u32,
    header: bool,
) -> Result<(), String> {
    if guard.iter().any(|saved| saved.hwnd == handle as isize) {
        return Ok(());
    }

    let child = handle as HWND;
    if fme_pid(child) != Some(pid) || GetAncestor(child, GA_ROOT) != child {
        return Err("Выбранное окно FME уже недоступно. Обновите список.".into());
    }
    let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
    placement.length = size_of::<WINDOWPLACEMENT>() as u32;
    if GetWindowPlacement(child, &mut placement) == 0 {
        return Err(failure("Не удалось сохранить положение"));
    }
    let mut rect: RECT = std::mem::zeroed();
    if GetWindowRect(child, &mut rect) == 0 {
        return Err(failure("Не удалось сохранить размеры окна"));
    }
    let parent = window.hwnd().map_err(|e| e.to_string())?.0 as HWND;
    let style = GetWindowLongPtrW(child, GWL_STYLE);
    let browser = FindWindowExW(
        parent,
        std::ptr::null_mut(),
        windows_sys::w!("WRY_WEBVIEW"),
        std::ptr::null(),
    );
    if browser.is_null() {
        return Err("Не найдено окно WebView".into());
    }
    let browser_style = GetWindowLongPtrW(browser, GWL_STYLE);
    let mut client_rect: RECT = std::mem::zeroed();
    GetClientRect(child, &mut client_rect);
    let header_height = if header {
        (client_rect.bottom
            + if GetMenu(child).is_null() {
                0
            } else {
                GetSystemMetrics(SM_CYMENU)
            })
        .max(1)
    } else {
        0
    };
    guard.push(Attached {
        hwnd: child as isize,
        pid,
        parent: GetParent(child) as isize,
        owner: GetWindow(child, GW_OWNER) as isize,
        style,
        placement,
        rect,
        browser: browser as isize,
        browser_style,
        header_height,
        layout: None,
    });
    // Keep the saved state even on partial failure so detach can restore the original window.
    set_style(
        child,
        (style & !((WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_MINIMIZE | WS_MAXIMIZE) as isize))
            | WS_CHILD as isize,
    )?;
    set_parent(child, parent)?;
    // WebView must clip sibling HWNDs or its compositor paints over the native client.
    set_style(browser, browser_style | WS_CLIPSIBLINGS as isize)?;

    Ok(())
}

#[tauri::command]
fn fme_attach(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Client>,
    hwnd: usize,
    pid: u32,
) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "Window state is unavailable")?;
    if !guard.is_empty() {
        return Err("Detach the current client first".into());
    }
    unsafe {
        if fme_pid(hwnd as HWND) != Some(pid) || IsWindowEnabled(hwnd as HWND) == 0 {
            return Err("The selected FME window is unavailable or has a modal dialog".into());
        }
    }
    let forms = process_forms(pid)?;
    if !forms.iter().any(|(handle, _)| *handle == hwnd) {
        return Err("The selected window is not a document form".into());
    }
    for (handle, header) in forms {
        unsafe {
            attach_window(&window, &mut guard, handle, pid, header)?;
        }
    }
    unsafe {
        BringWindowToTop(hwnd as HWND);
    }
    Ok(())
}

fn physical_bounds(bounds: [f64; 4], scale: f64, size: (u32, u32)) -> Result<[i32; 4], String> {
    let [x, y, width, height] = bounds.map(|v| (v * scale).round());
    if ![x, y, width, height]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= i32::MAX as f64)
        || width < 1.0
        || height < 1.0
        || x + width > size.0 as f64 + 1.0
        || y + height > size.1 as f64 + 1.0
    {
        return Err("Некорректные размеры панели".into());
    }
    Ok([x as i32, y as i32, width as i32, height as i32])
}

#[tauri::command]
fn fme_bounds(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Client>,
    bounds: [f64; 4],
    visible: bool,
) -> Result<usize, String> {
    let size = window.inner_size().map_err(|e| e.to_string())?;
    let [x, y, w, h] = physical_bounds(
        bounds,
        window.scale_factor().map_err(|e| e.to_string())?,
        (size.width, size.height),
    )?;
    let host = window.hwnd().map_err(|e| e.to_string())?.0 as HWND;
    let mut guard = state.0.lock().map_err(|_| "Window state is unavailable")?;
    let Some(pid) = guard.first().map(|item| item.pid) else {
        return Ok(0);
    };
    unsafe {
        guard.retain(|saved| {
            saved.header_height > 0
                || (IsWindow(saved.hwnd as HWND) != 0
                    && fme_pid(saved.hwnd as HWND) == Some(pid)
                    && GetParent(saved.hwnd as HWND) == host)
        });
    }
    for (handle, header) in process_forms(pid)? {
        unsafe {
            attach_window(&window, &mut guard, handle, pid, header)?;
        }
    }
    let mut origin = POINT { x: 0, y: 0 };
    unsafe {
        if ClientToScreen(host, &mut origin) == 0 {
            return Err(failure("Could not locate host window"));
        }
    }
    for saved in guard
        .iter_mut()
        .filter(|item| item.header_height > 0 && item.layout.is_some())
    {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(saved.hwnd as HWND, &mut rect) } != 0 {
            let height = rect.bottom - rect.top;
            if height > 0 && height < h / 2 {
                saved.header_height = height;
            }
        }
    }
    let header_height = guard
        .iter()
        .map(|item| item.header_height)
        .sum::<i32>()
        .min(h - 1);
    for saved in guard.iter_mut() {
        let (y, h) = if saved.header_height > 0 {
            (y, header_height)
        } else {
            (y + header_height, h - header_height)
        };
        let [x, y, w, h] = view_bounds([x, y, w, h], visible);
        unsafe {
            let hwnd = saved.hwnd as HWND;
            if IsWindow(hwnd) == 0 || fme_pid(hwnd) != Some(pid) {
                continue;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let embedded = (style
                & !((WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_MINIMIZE | WS_MAXIMIZE) as isize))
                | WS_CHILD as isize;
            if style != embedded {
                set_style(hwnd, embedded)?;
            }
            let mut rect: RECT = std::mem::zeroed();
            if GetWindowRect(hwnd, &mut rect) == 0 {
                continue;
            }
            if [
                rect.left - origin.x,
                rect.top - origin.y,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ] != [x, y, w, h]
                || style != embedded
            {
                // FME owns visibility and tab order; layout must never raise or show every form.
                if SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    x,
                    y,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                ) == 0
                {
                    return Err(failure("Could not resize embedded form"));
                }
            }
            saved.layout = Some([x, y, w, h]);
        }
    }
    unsafe {
        if visible && IsIconic(host) == 0 {
            let context = (
                pid,
                [
                    origin.x + x,
                    origin.y + y + header_height,
                    w,
                    h - header_height,
                ],
            );
            EnumWindows(Some(contain_dialog), &context as *const _ as LPARAM);
        }
    }
    Ok(guard.iter().filter(|item| item.header_height == 0).count())
}

fn view_bounds([x, y, w, h]: [i32; 4], visible: bool) -> [i32; 4] {
    // Keep native visibility and tab order intact while another Minerva view is open.
    if visible {
        [x, y, w, h]
    } else {
        [-w, -h, w, h]
    }
}

unsafe fn read_menu(menu: HMENU) -> Vec<serde_json::Value> {
    let mut result = Vec::new();
    for index in 0..GetMenuItemCount(menu) {
        let mut title = [0u16; 256];
        let length = GetMenuStringW(
            menu,
            index as u32,
            title.as_mut_ptr(),
            title.len() as i32,
            MF_BYPOSITION,
        );
        result.push(serde_json::json!({"index":index,"title":String::from_utf16_lossy(&title[..length as usize]).replace('&',""),"enabled":GetMenuState(menu,index as u32,MF_BYPOSITION) & (MF_DISABLED | MF_GRAYED) == 0}));
    }
    result
}

#[tauri::command]
fn fme_menu(state: tauri::State<'_, Client>) -> Result<Vec<serde_json::Value>, String> {
    let guard = state.0.lock().map_err(|_| "Состояние окна недоступно")?;
    let frame = guard
        .iter()
        .find(|item| item.header_height > 0)
        .ok_or("Главное меню FME не подключено")?;
    unsafe {
        let hwnd = frame.hwnd as HWND;
        if fme_pid(hwnd) != Some(frame.pid) {
            return Err("Окно меню закрыто".into());
        }
        let menu = client_menu(hwnd, frame.pid);
        Ok(read_menu(menu))
    }
}

#[tauri::command]
fn fme_menu_open(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Client>,
    index: i32,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let size = window.inner_size().map_err(|e| e.to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let [x, y, _, _] = physical_bounds([x, y, 1., 1.], scale, (size.width, size.height))?;
    let (hwnd, pid) = {
        let guard = state.0.lock().map_err(|_| "Состояние окна недоступно")?;
        let frame = guard
            .iter()
            .find(|item| item.header_height > 0)
            .ok_or("Главное меню FME не подключено")?;
        (frame.hwnd as HWND, frame.pid)
    };
    unsafe {
        if fme_pid(hwnd) != Some(pid) {
            return Err("Окно меню закрыто".into());
        }
        let menu = client_menu(hwnd, pid);
        if index < 0 || index >= GetMenuItemCount(menu) {
            return Err("Пункт меню недоступен".into());
        }
        let submenu = GetSubMenu(menu, index);
        if submenu.is_null() {
            return Err("Подменю недоступно".into());
        }
        SendMessageW(hwnd, WM_INITMENU, menu as usize, 0);
        SendMessageW(hwnd, WM_INITMENUPOPUP, submenu as usize, index as isize);
        let mut point = POINT { x, y };
        let parent = window.hwnd().map_err(|e| e.to_string())?.0 as HWND;
        if ClientToScreen(parent, &mut point) == 0 {
            return Err(failure("Не удалось определить положение меню"));
        }
        let labels = delphi_menu_labels(hwnd, pid);
        let popup = clone_menu(submenu, 0, &labels)?;
        SetLastError(0);
        let command = TrackPopupMenuEx(
            popup,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_NONOTIFY,
            point.x,
            point.y,
            parent,
            std::ptr::null(),
        );
        let error = GetLastError();
        DestroyMenu(popup);
        if command == 0 && error != 0 {
            return Err(format!(
                "Не удалось открыть меню: {}",
                std::io::Error::from_raw_os_error(error as i32)
            ));
        }
        if command != 0 {
            if PostMessageW(hwnd, WM_COMMAND, command as usize, 0) == 0 {
                return Err(failure("Could not dispatch FME command"));
            }
        }
    }
    Ok(())
}

unsafe fn clone_menu(original: HMENU, depth: usize, labels: &MenuLabels) -> Result<HMENU, String> {
    let count = GetMenuItemCount(original);
    if depth > 8 || !(0..=500).contains(&count) {
        return Err("Недопустимая структура меню".into());
    }
    let popup = CreatePopupMenu();
    if popup.is_null() {
        return Err(failure("Не удалось создать подменю"));
    }
    let result = (|| {
        for index in 0..count {
            let state = GetMenuState(original, index as u32, MF_BYPOSITION);
            let mut flags = state & (MF_DISABLED | MF_GRAYED | MF_CHECKED | MF_SEPARATOR);
            let submenu = GetSubMenu(original, index);
            let child = if submenu.is_null() {
                std::ptr::null_mut()
            } else {
                clone_menu(submenu, depth + 1, labels)?
            };
            let id = if child.is_null() {
                GetMenuItemID(original, index) as usize
            } else {
                flags |= MF_POPUP;
                child as usize
            };
            let mut title = [0u16; 1024];
            GetMenuStringW(
                original,
                index as u32,
                title.as_mut_ptr(),
                title.len() as i32,
                MF_BYPOSITION,
            );
            let caption = if title[0] == 0 && flags & MF_SEPARATOR == 0 {
                if submenu.is_null() {
                    labels.commands.get(&(id as u32))
                } else {
                    labels.popups.get(&(submenu as usize))
                }
            } else {
                None
            };
            if AppendMenuW(
                popup,
                flags,
                id,
                caption.map_or(title.as_ptr(), |text| text.as_ptr()),
            ) == 0
            {
                if !child.is_null() {
                    DestroyMenu(child);
                }
                return Err(failure("Не удалось скопировать пункт меню"));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        DestroyMenu(popup);
        return Err(error);
    }
    Ok(popup)
}

pub(super) fn handle(invoke: tauri::ipc::Invoke) -> bool {
    let handler: Box<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool> =
        Box::new(tauri::generate_handler![
            fme_windows,
            fme_show,
            fme_attach,
            fme_bounds,
            fme_detach,
            fme_connection,
            fme_menu,
            fme_menu_open
        ]);
    handler(invoke)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_view_moves_forms_outside_host_without_changing_size() {
        assert_eq!(
            view_bounds([252, 36, 900, 600], false),
            [-900, -600, 900, 600]
        );
        assert_eq!(view_bounds([252, 36, 900, 600], true), [252, 36, 900, 600]);
    }
    #[test]
    fn dialogs_stay_inside_the_client_area_without_clipping_large_dialogs() {
        assert_eq!(
            dialog_position([0, 0, 300, 200], [250, 350, 1000, 600]),
            Some((600, 550))
        );
        assert_eq!(
            dialog_position([300, 400, 600, 600], [250, 350, 1000, 600]),
            None
        );
        assert_eq!(
            dialog_position([0, 0, 1300, 900], [250, 350, 1000, 600]),
            None
        );
    }
    #[test]
    fn tab_forms_include_hidden_documents_but_exclude_helpers_and_popups() {
        let document = WS_OVERLAPPEDWINDOW;
        assert!(is_tab_form("TDocument", document));
        assert!(is_tab_form("TDocument", document | WS_VISIBLE));
        assert!(is_tab_form("TMainFrame", document));
        assert!(!is_tab_form("TApplication", document));
        assert!(!is_tab_form("TDialog", document | WS_POPUP));
        assert!(!is_tab_form("TControl", document | WS_CHILD));
        assert!(!is_tab_form("TDialog", WS_CAPTION | WS_SYSMENU));
    }
    #[test]
    #[ignore = "Requires a running FME client"]
    fn live_process_forms_include_every_visible_tab_candidate() {
        let windows = fme_windows().unwrap();
        let frame = windows
            .iter()
            .find(|item| item["class"] == "TMainFrame")
            .expect("No unembedded FME main frame found");
        let pid = frame["pid"].as_u64().unwrap() as u32;
        let forms = process_forms(pid).unwrap();
        assert!(forms.iter().any(|(_, header)| *header));
        for item in windows
            .iter()
            .filter(|item| item["pid"].as_u64() == Some(pid as u64))
        {
            let hwnd = item["hwnd"].as_u64().unwrap() as HWND;
            if unsafe {
                is_tab_form(
                    item["class"].as_str().unwrap(),
                    GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
                )
            } {
                assert!(forms.iter().any(|(handle, _)| *handle == hwnd as usize));
            }
        }
        assert!(forms.len() > 1);
    }
    #[test]
    fn remote_reads_are_bounded_and_handle_invalid_addresses() {
        unsafe {
            let process = GetCurrentProcess();
            let value = 1234usize;
            assert_eq!(
                remote_pointer(process, &value as *const _ as usize),
                Some(value)
            );
            assert!(remote_bytes(process, 0, 8).is_none());
            assert!(remote_bytes(process, &value as *const _ as usize, 16385).is_none());
        }
    }
    #[test]
    #[ignore = "Requires a running FME 3.7 x64 client"]
    fn live_delphi_main_menu() {
        unsafe extern "system" fn include_children(hwnd: HWND, data: LPARAM) -> BOOL {
            enumerate(hwnd, data);
            EnumChildWindows(hwnd, Some(enumerate), data);
            1
        }
        unsafe {
            let mut windows: Vec<serde_json::Value> = Vec::new();
            EnumWindows(Some(include_children), &mut windows as *mut _ as LPARAM);
            let frames = windows.iter().filter(|item| item["class"] == "TMainFrame");
            let mut found = false;
            for frame in frames {
                let menu = delphi_menu(
                    frame["hwnd"].as_u64().unwrap() as HWND,
                    frame["pid"].as_u64().unwrap() as u32,
                );
                if !menu.is_null() {
                    assert!(read_menu(menu).len() >= 2);
                    let labels = delphi_menu_labels(
                        frame["hwnd"].as_u64().unwrap() as HWND,
                        frame["pid"].as_u64().unwrap() as u32,
                    );
                    assert!(!labels.commands.is_empty());
                    let help = read_menu(menu)
                        .into_iter()
                        .find(|item| item["title"] == "Справка")
                        .unwrap();
                    let copy = clone_menu(
                        GetSubMenu(menu, help["index"].as_i64().unwrap() as i32),
                        0,
                        &labels,
                    )
                    .unwrap();
                    let items = read_menu(copy);
                    DestroyMenu(copy);
                    assert!(!items[0]["title"].as_str().unwrap().is_empty());
                    found = true;
                }
            }
            assert!(found, "No validated Delphi main menu found");
        }
    }
    #[test]
    fn native_menu_titles_and_disabled_items() {
        unsafe {
            let menu = CreateMenu();
            assert!(!menu.is_null());
            assert_ne!(AppendMenuW(menu, MF_STRING, 1, windows_sys::w!("&Help")), 0);
            assert_ne!(
                AppendMenuW(menu, MF_STRING | MF_GRAYED, 2, windows_sys::w!("Disabled")),
                0
            );
            let items = read_menu(menu);
            let copy = clone_menu(menu, 0, &MenuLabels::default()).unwrap();
            assert_eq!(read_menu(copy), items);
            assert_eq!(GetMenuItemID(copy, 0), 1);
            DestroyMenu(copy);
            DestroyMenu(menu);
            assert_eq!(items[0]["title"], "Help");
            assert_eq!(items[0]["enabled"], true);
            assert_eq!(items[1]["enabled"], false);
        }
    }
    #[test]
    #[ignore = "Requires a running FME client"]
    fn live_window_enumeration() {
        let windows = fme_windows().unwrap();
        println!("{windows:?}");
        assert!(!windows.is_empty(), "No FME forms found");
    }
    #[test]
    fn bounds_follow_dpi_and_reject_invalid_input() {
        assert_eq!(
            physical_bounds([0., 120., 800., 400.], 1.5, (1200, 780)).unwrap(),
            [0, 180, 1200, 600]
        );
        for bounds in [
            [0., 0., f64::NAN, 1.],
            [-1., 0., 1., 1.],
            [0., 0., 0., 1.],
            [0., 0., 802., 400.],
        ] {
            assert!(physical_bounds(bounds, 1., (800, 600)).is_err());
        }
    }
}
