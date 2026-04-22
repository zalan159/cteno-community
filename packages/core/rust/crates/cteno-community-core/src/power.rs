use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "tauri-commands")]
pub mod commands;

static POWER_ASSERTION_ID: AtomicU32 = AtomicU32::new(0);
static ASSERTION_COUNT: AtomicU32 = AtomicU32::new(0);

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    type CFStringRef = *const std::ffi::c_void;
    type CFAllocatorRef = *const std::ffi::c_void;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const i8,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: u32) -> i32;
    }

    fn create_cfstring(s: &str) -> Option<CFStringRef> {
        let c_str = std::ffi::CString::new(s).ok()?;
        let cf_str = unsafe {
            CFStringCreateWithCString(std::ptr::null(), c_str.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if cf_str.is_null() {
            None
        } else {
            Some(cf_str)
        }
    }

    pub fn prevent_sleep_impl(reason: &str) -> Result<(), String> {
        let count = ASSERTION_COUNT.fetch_add(1, Ordering::SeqCst);
        if count > 0 {
            log::debug!("Power assertion already active, count now: {}", count + 1);
            return Ok(());
        }

        let assertion_type = create_cfstring("NoIdleSleepAssertion")
            .ok_or_else(|| "Failed to create assertion type CFString".to_string())?;
        let assertion_name = create_cfstring(&format!("Cteno: {}", reason)).ok_or_else(|| {
            unsafe { CFRelease(assertion_type) };
            "Failed to create assertion name CFString".to_string()
        })?;
        let mut assertion_id: u32 = 0;
        let result = unsafe {
            IOPMAssertionCreateWithName(
                assertion_type,
                K_IOPM_ASSERTION_LEVEL_ON,
                assertion_name,
                &mut assertion_id,
            )
        };
        unsafe {
            CFRelease(assertion_type);
            CFRelease(assertion_name);
        }
        if result == 0 {
            POWER_ASSERTION_ID.store(assertion_id, Ordering::SeqCst);
            Ok(())
        } else {
            ASSERTION_COUNT.fetch_sub(1, Ordering::SeqCst);
            Err(format!(
                "Failed to create power assertion: error code {}",
                result
            ))
        }
    }

    pub fn allow_sleep_impl() -> Result<(), String> {
        let count = ASSERTION_COUNT.fetch_sub(1, Ordering::SeqCst);
        if count > 1 {
            return Ok(());
        }
        if count == 0 {
            ASSERTION_COUNT.store(0, Ordering::SeqCst);
            return Ok(());
        }
        let assertion_id = POWER_ASSERTION_ID.swap(0, Ordering::SeqCst);
        if assertion_id == 0 {
            return Ok(());
        }
        let result = unsafe { IOPMAssertionRelease(assertion_id) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "Failed to release power assertion: error code {}",
                result
            ))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub fn prevent_sleep_impl(_reason: &str) -> Result<(), String> {
        ASSERTION_COUNT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn allow_sleep_impl() -> Result<(), String> {
        let count = ASSERTION_COUNT.fetch_sub(1, Ordering::SeqCst);
        if count == 0 {
            ASSERTION_COUNT.store(0, Ordering::SeqCst);
        }
        Ok(())
    }
}

pub fn prevent_sleep(reason: &str) -> Result<(), String> {
    platform::prevent_sleep_impl(reason)
}

pub fn allow_sleep() -> Result<(), String> {
    platform::allow_sleep_impl()
}

pub fn is_sleep_prevented() -> bool {
    POWER_ASSERTION_ID.load(Ordering::SeqCst) != 0
}

pub struct PowerGuard {
    reason: String,
}

impl PowerGuard {
    pub fn new(reason: &str) -> Result<Self, String> {
        prevent_sleep(reason)?;
        Ok(Self {
            reason: reason.to_string(),
        })
    }
}

impl Drop for PowerGuard {
    fn drop(&mut self) {
        if let Err(e) = allow_sleep() {
            log::error!("Failed to release power guard '{}': {}", self.reason, e);
        }
    }
}
