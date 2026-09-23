#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

const MMAP_THRESHOLD: core::ffi::c_int = -3;

const TRIM_THRESHOLD: core::ffi::c_int = -1;

const LARGE: core::ffi::c_int = 128 * 1024;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn mallopt(param: core::ffi::c_int, value: core::ffi::c_int) -> core::ffi::c_int;

    fn malloc_trim(pad: usize) -> core::ffi::c_int;
}

#[must_use]
pub fn settle() -> bool {
    #[cfg(target_os = "linux")]
    {
        unsafe {
            mallopt(MMAP_THRESHOLD, LARGE);
            mallopt(TRIM_THRESHOLD, LARGE);
        }
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[must_use]
pub fn give_back() -> bool {
    #[cfg(target_os = "linux")]
    {
        unsafe { malloc_trim(0) != 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[must_use]
pub const fn what_it_does() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "glibc: 128 KiB mmap and trim thresholds, and a trim when a document is let go"
    }
    #[cfg(target_os = "windows")]
    {
        "Windows: nothing yet -- the allocator has not been measured there (RFC 0014)"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS: nothing yet -- the allocator has not been measured there (RFC 0014)"
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        "nothing: this platform's allocator has not been looked at"
    }
}

#[cfg(test)]
mod tests {
    use super::{LARGE, MMAP_THRESHOLD, TRIM_THRESHOLD, give_back, settle, what_it_does};

    #[test]
    fn the_parameters_are_the_ones_glibc_names() {
        assert_eq!(MMAP_THRESHOLD, -3);
        assert_eq!(TRIM_THRESHOLD, -1);
        assert_ne!(MMAP_THRESHOLD, TRIM_THRESHOLD);
        assert_eq!(LARGE, 131_072);
    }

    #[test]
    fn asking_twice_is_not_worse_than_asking_once() {
        let first = settle();
        let second = settle();
        assert_eq!(first, second);
        let _ = give_back();
        let _ = give_back();
    }

    #[test]
    fn it_says_what_it_does_wherever_it_is_built() {
        let said = what_it_does();
        assert!(!said.is_empty());
        #[cfg(target_os = "linux")]
        assert!(settle(), "on Linux it does something");
        #[cfg(not(target_os = "linux"))]
        {
            assert!(!settle(), "elsewhere it does nothing");
            assert!(said.contains("nothing"), "and it says so: {said}");
        }
    }
}
