//! The cameras Media Foundation lists, by the friendly name `dshow` also opens them by.

use laqta_ports::PortError;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MFCreateAttributes, MFEnumDeviceSources,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::PWSTR;

use crate::loopback::Mta;

/// Camera names, the system's first camera first. This blocks, so run it off the async worker.
pub(crate) fn list() -> Result<Vec<String>, PortError> {
    let failed = |step: &'static str| move |e: windows::core::Error| PortError::Failed(format!("{step}: {e}"));
    let _com = Mta::enter();
    let mut attributes: Option<IMFAttributes> = None;
    // SAFETY: `attributes` is a valid out pointer.
    unsafe { MFCreateAttributes(&raw mut attributes, 1) }.map_err(failed("camera list"))?;
    let attributes = attributes.ok_or_else(|| PortError::Failed("camera list: no attribute store".into()))?;
    // SAFETY: both GUIDs are constants that live for the whole call.
    unsafe { attributes.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID) }
        .map_err(failed("camera list"))?;
    let mut sources: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: on success `sources` points to `count` activation objects in one CoTaskMem block.
    unsafe { MFEnumDeviceSources(&attributes, &raw mut sources, &raw mut count) }.map_err(failed("cameras"))?;
    let mut names = Vec::new();
    for index in 0..count as usize {
        // SAFETY: `index < count`. Taking the object out of the block releases it when it drops.
        let Some(source) = (unsafe { (*sources.add(index)).take() }) else { continue };
        let mut text = PWSTR::null();
        let mut len = 0u32;
        // SAFETY: valid out pointers. The string is CoTaskMem memory, freed below.
        if unsafe { source.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, &raw mut text, &raw mut len) }.is_ok()
            && !text.is_null()
        {
            // SAFETY: `text` holds `len` UTF-16 units.
            names.push(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text.0, len as usize) }));
            // SAFETY: allocated by Media Foundation with CoTaskMemAlloc.
            unsafe { CoTaskMemFree(Some(text.0.cast())) };
        }
    }
    // SAFETY: the block from `MFEnumDeviceSources`; every object in it was taken above. Null is allowed.
    unsafe { CoTaskMemFree(Some(sources.cast())) };
    Ok(names)
}
