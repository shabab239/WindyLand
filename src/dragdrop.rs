//! OLE drag & drop: files dragged onto the notch (in) and shelf items dragged
//! out into Explorer, chats, mail, etc.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::*;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;

use crate::app::with_app;

#[implement(IDropTarget)]
struct Target;

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, _d: Option<&IDataObject>, _k: MODIFIERKEYS_FLAGS, _pt: &POINTL, eff: *mut DROPEFFECT) -> Result<()> {
        let ok = with_app(|a| {
            a.drag_enter();
            !a.dragging_out
        })
        .unwrap_or(false);
        unsafe { *eff = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragOver(&self, _k: MODIFIERKEYS_FLAGS, _pt: &POINTL, eff: *mut DROPEFFECT) -> Result<()> {
        let ok = with_app(|a| !a.dragging_out).unwrap_or(false);
        unsafe { *eff = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        with_app(|a| a.drag_leave());
        Ok(())
    }

    fn Drop(&self, d: Option<&IDataObject>, _k: MODIFIERKEYS_FLAGS, _pt: &POINTL, eff: *mut DROPEFFECT) -> Result<()> {
        let paths = d.map(|d| unsafe { extract(d) }).unwrap_or_default();
        with_app(|a| a.drop_paths(paths));
        unsafe { *eff = DROPEFFECT_COPY };
        Ok(())
    }
}

pub fn register(hwnd: HWND) {
    let t: IDropTarget = Target.into();
    unsafe {
        let _ = RegisterDragDrop(hwnd, &t);
    }
}

fn fmt(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// Paths from CF_HDROP, or a .url shortcut for links dragged from a browser.
unsafe fn extract(d: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut med) = d.GetData(&fmt(CF_HDROP.0)) {
        let h = HDROP(med.u.hGlobal.0);
        let n = DragQueryFileW(h, 0xFFFF_FFFF, None);
        for i in 0..n {
            let len = DragQueryFileW(h, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(h, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut med);
    }
    if out.is_empty() {
        let cf = RegisterClipboardFormatW(w!("UniformResourceLocatorW")) as u16;
        if let Ok(mut med) = d.GetData(&fmt(cf)) {
            let hg = med.u.hGlobal;
            let p = GlobalLock(hg) as *const u16;
            if !p.is_null() {
                let mut len = 0;
                while *p.add(len) != 0 && len < 8192 {
                    len += 1;
                }
                let url = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                let _ = GlobalUnlock(hg);
                if let Some(path) = save_url(&url) {
                    out.push(path);
                }
            }
            ReleaseStgMedium(&mut med);
        }
    }
    out
}

/// Store a dropped link as an Internet Shortcut so it can live on the shelf.
fn save_url(url: &str) -> Option<String> {
    let dir = crate::store::dir().join("links");
    std::fs::create_dir_all(&dir).ok()?;
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("link")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-', "_");
    let mut p = dir.join(format!("{host}.url"));
    let mut k = 2;
    while p.exists() {
        p = dir.join(format!("{host} ({k}).url"));
        k += 1;
    }
    std::fs::write(&p, format!("[InternetShortcut]\r\nURL={url}\r\n")).ok()?;
    Some(p.display().to_string())
}

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, esc: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if esc.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _e: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Start a modal OLE drag of real files (Explorer-compatible data object).
pub fn drag_out(paths: &[String]) {
    unsafe {
        let pidls: Vec<*mut ITEMIDLIST> = paths
            .iter()
            .map(|p| ILCreateFromPathW(&HSTRING::from(p.as_str())))
            .filter(|p| !p.is_null())
            .collect();
        if pidls.is_empty() {
            return;
        }
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        if let Ok(arr) = SHCreateShellItemArrayFromIDLists(&consts) {
            if let Ok(obj) = arr.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) {
                let src: IDropSource = Source.into();
                let mut eff = DROPEFFECT(0);
                let _ = DoDragDrop(&obj, &src, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut eff);
            }
        }
        for p in pidls {
            ILFree(Some(p as *const _));
        }
    }
}
