//! Dragging files out of the window into Explorer, the desktop or any app
//! that takes dropped files. The shell builds the data object from the
//! files (the same one Explorer uses for its own drags) and supplies the
//! drag feedback, so every drop target understands it.

use std::path::PathBuf;

use anyhow::{Result, bail};
use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::IDataObject,
            Ole::{DROPEFFECT_COPY, DROPEFFECT_NONE, IDropSource},
        },
        UI::Shell::{BHID_DataObject, Common::ITEMIDLIST, ILCreateFromPathW, ILFree, SHCreateShellItemArrayFromIDLists, SHDoDragDrop},
    },
    core::HSTRING,
};

/// Start a copy-drag of `files`; returns once the drop happened or was
/// cancelled (true if something took the files). Call while the left button
/// is down, on the window's thread (OLE is initialised there by winit).
pub fn drag_files(hwnd: isize, files: &[PathBuf]) -> Result<bool> {
    if files.is_empty() {
        bail!("nothing to drag");
    }
    unsafe {
        let pidls: Vec<*mut ITEMIDLIST> = files.iter().map(|f| ILCreateFromPathW(&HSTRING::from(f.as_os_str()))).collect();
        let ok = pidls.iter().all(|p| !p.is_null());
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let array = if ok { Some(SHCreateShellItemArrayFromIDLists(&consts)) } else { None };
        for p in pidls.into_iter().filter(|p| !p.is_null()) {
            ILFree(Some(p));
        }
        let Some(array) = array else { bail!("a copied file is missing") };
        let data: IDataObject = array?.BindToHandler(None, &BHID_DataObject)?;
        let effect = SHDoDragDrop(Some(HWND(hwnd as _)), &data, None::<&IDropSource>, DROPEFFECT_COPY)?;
        Ok(effect != DROPEFFECT_NONE)
    }
}
