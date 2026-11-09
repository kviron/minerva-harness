import React, {useEffect, useRef, useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Button} from '../components/ui/button';
import {Plug, Unplug, RefreshCw, PanelTopOpen, X} from 'lucide-react';
import '../style.css';
import './native-client.css';

type ClientWindow = {hwnd: number; pid: number; title: string; class: string};
type ClientMenu = {index:number; title:string; enabled:boolean};

export function NativeClientExperiment({active}:{active:boolean}) {
 const [windows,setWindows]=useState<ClientWindow[]>([]);
 const [selected,setSelected]=useState('');
 const [attached,setAttached]=useState(false);
 const [menu,setMenu]=useState<ClientMenu[]>([]);
 const [busy,setBusy]=useState(false);
 const [error,setError]=useState('');
 const panel=useRef<HTMLDivElement>(null);
 const clients=new Map<number,ClientWindow>();
 for(const item of windows)if(!clients.has(item.pid))clients.set(item.pid,item);
 const selectedClient=windows.find(item=>String(item.hwnd)===selected);
 async function refresh() {
  const [available,connection]=await Promise.all([invoke<ClientWindow[]>('fme_windows'),invoke<ClientWindow|null>('fme_connection')]);
  const list=available.filter(item=>item.class!=='TMainFrame');
  if(connection&&!list.some(item=>item.hwnd===connection.hwnd))list.unshift(connection);
  setWindows(list);
  setAttached(!!connection);
  setSelected(current=>list.some(item=>String(item.hwnd)===current)?current:String(list[0]?.hwnd??''));
 }
 async function action(task:()=>Promise<void>) {
  setBusy(true);setError('');
  try { await task(); } catch(error) { setError(String(error)); }
  finally { setBusy(false); }
 }
 useEffect(()=>{void action(refresh);},[]);
 useEffect(()=>{
  if(!attached){setMenu([]);return;}
  let active=true;
  void invoke<ClientMenu[]>('fme_menu').then(items=>{if(active)setMenu(items);}).catch(error=>{if(active)setError(String(error));});
  return ()=>{active=false;};
 },[attached]);
 useEffect(()=>{
  if(!attached||!panel.current)return;
  let pending=false;
  let alive=true;
  const resize=async()=>{
   if(pending||!alive)return;
   const rect=(active?panel.current:panel.current?.closest('main'))?.getBoundingClientRect();
   if(!rect||rect.width<1||rect.height<1)return;
   const bounds=[rect.x,rect.y,rect.width,rect.height];
   pending=true;
   try {await invoke('fme_bounds',{bounds,visible:active});}
   catch(error){if(alive)setError(String(error));}
   finally{pending=false;}
  };
  const observer=new ResizeObserver(resize);
  observer.observe(panel.current);window.addEventListener('resize',resize);resize();
  const timer=setInterval(()=>void resize(),150);
  return ()=>{alive=false;clearInterval(timer);observer.disconnect();window.removeEventListener('resize',resize);};
 },[attached,active]);
 async function attach() {
  const target=windows.find(item=>String(item.hwnd)===selected);
  if(!target)throw Error('Выберите окно FME');
  try {
   await invoke('fme_attach',{hwnd:target.hwnd,pid:target.pid});
   setAttached(true);
  } catch(error) {
   try { await invoke('fme_detach'); }
   catch(restoreError) {setAttached(true);throw Error(`${String(error)}. Восстановление: ${String(restoreError)}`);}
   throw error;
  }
 }
 return <section className="oe-studio fme-experiment" style={active?undefined:{display:'none'}} aria-hidden={!active}>
  <div className="fme-controls" aria-label="Подключение клиента">
   <span className="fme-connection-dot" data-connected={attached} aria-hidden="true"/>
   <select id="fme-window" aria-label="Клиент Восточного Экспресса" title={attached?'Клиент подключён':'Выбрать процесс FME'} value={selectedClient?.pid??''} onChange={event=>setSelected(String(clients.get(Number(event.target.value))?.hwnd??''))} disabled={busy||attached}>
    {!clients.size&&<option value="">Нет FME</option>}
    {[...clients.values()].map(item=><option key={item.pid} value={item.pid}>FME {item.pid}</option>)}
   </select>
   {!attached&&<Button variant="ghost" size="icon" className="fme-control-button" title="Обновить список" aria-label="Обновить список клиентов" disabled={busy} onClick={()=>void action(refresh)}><RefreshCw/></Button>}
   {!clients.size&&<Button variant="ghost" size="icon" className="fme-control-button" title="Показать FME" aria-label="Показать FME" disabled={busy} onClick={()=>void action(async()=>{await invoke('fme_show');await new Promise(resolve=>setTimeout(resolve,500));await refresh();})}><PanelTopOpen/></Button>}
   <Button variant="ghost" size="icon" className="fme-control-button" title={attached?'Отсоединить клиент':'Встроить клиент'} aria-label={attached?'Отсоединить клиент':'Встроить клиент'} disabled={busy||(!attached&&!selected)} onClick={()=>void action(attached?async()=>{await invoke('fme_detach');setAttached(false);await refresh();}:attach)}>{attached?<Unplug/>:<Plug/>}</Button>
  </div>
  {attached&&menu.length>0&&<nav className="fme-menubar" aria-label="Главное меню Восточного Экспресса">{menu.map(item=><Button key={item.index} variant="ghost" disabled={busy||!item.enabled} onClick={event=>{const rect=event.currentTarget.getBoundingClientRect();void action(async()=>{await invoke('fme_menu_open',{index:item.index,x:rect.left,y:rect.bottom});});}}>{item.title}</Button>)}</nav>}
  {error&&<div id="fme-error" className="fme-error" role="alert"><span>{error}</span><Button variant="ghost" size="icon" className="fme-control-button" aria-label="Закрыть сообщение" onClick={()=>setError('')}><X/></Button></div>}
  <div ref={panel} className="fme-surface" aria-label="Область нативного клиента">
   {!attached&&<div className="fme-empty">Подключите клиент в правом верхнем углу</div>}
  </div>
 </section>;
}
