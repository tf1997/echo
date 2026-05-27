#[cfg(windows)]
pub fn write_error(message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::System::EventLog::{
        DeregisterEventSource, RegisterEventSourceW, ReportEventW, EVENTLOG_ERROR_TYPE,
    };

    fn wide_null(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }

    let source = wide_null("Echo");
    let message = wide_null(message);

    unsafe {
        let handle = RegisterEventSourceW(ptr::null(), source.as_ptr());
        if handle == 0 {
            return;
        }

        let strings = [message.as_ptr()];
        let _ = ReportEventW(
            handle,
            EVENTLOG_ERROR_TYPE,
            0,
            1000,
            ptr::null_mut(),
            strings.len() as u16,
            0,
            strings.as_ptr(),
            ptr::null(),
        );
        let _ = DeregisterEventSource(handle);
    }
}

#[cfg(not(windows))]
pub fn write_error(_message: &str) {}
