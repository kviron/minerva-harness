# Minerva application artwork

`minerva-profile.png` is the current source artwork for application icons. The
native Windows, macOS, Linux, Android and iOS icons in `src-tauri/icons/` are
generated with:

```sh
npx tauri icon assets/minerva-profile.png
```

The PNG files and favicon in `apps/web/public/` are derived from the same source
for the browser, PWA, sidebar and welcome image. Rebuild the frontend after
updating them. Native icons are embedded at compile time, so rebuild the Tauri
application to update the executable, window and tray icons.

`new_icons/`, `minerva-master.png` and `minerva-macos.png` contain earlier artwork
and exports retained for reference.
