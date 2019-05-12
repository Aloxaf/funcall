use funcall::Func;
use std::ffi::CStr;

mod cdecl_func;

// test push with miri
#[test]
fn push() {
    let mut func = Func::from_raw(0 as *const fn());
    func.push(0u8);
    func.push(0i8);
    func.push(0u16);
    func.push(0i16);
    func.push(0u32);
    func.push(0i32);
    func.push(0isize);
    func.push(0usize);
    func.push(0i64);
    func.push(0u64);
    func.push(0u128);
    func.push(0i128);
    func.push(0.0f32);
    func.push(0.0f64);
    func.push(b"".as_ptr());
}

macro_rules! define_test {
    ($name: ident, $func: path, $arg: expr, $ret: ident) => {
        #[test]
        fn $name() {
            let mut func = Func::from_raw($func as *const fn());
            func.push($arg);
            unsafe {
                func.cdecl();
            }
            assert_eq!(func.$ret(), $arg);
        }
    };
}

mod cdecl {
    use super::*;

    #[test]
    fn more_than_6_args() {
        let mut func = Func::from_raw(cdecl_func::more_than_6_args as *const fn());
        for i in 1..=8 {
            func.push(i);
        }
        unsafe {
            func.cdecl();
            assert_eq!(func.ret_as_usize(), (1..=8).sum());
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn sprintf() {
        // FIXME: debug 模式下最后一个浮点数偶尔会变成 0.0
        for _ in 0..100 {
            let mut buf = vec![0i8; 100];
            let mut func = if cfg!(target_arch = "x86") {
                Func::new("/usr/lib32/libc.so.6", b"sprintf\0").unwrap()
            } else {
                Func::new("/usr/lib/libc.so.6", b"sprintf\0").unwrap()
            };
            func.push(buf.as_mut_ptr());
            func.push(b"%d %d %d %d %d %d %.4f\0".as_ptr());
            func.push(3i32);
            func.push(4i32);
            func.push(5i32);
            func.push(6i32);
            func.push(7i32);
            func.push(8i32);
            func.push(1234.5678f64);
            unsafe {
                func.cdecl();
                assert_eq!(
                    CStr::from_ptr(buf.as_ptr()).to_str().unwrap(),
                    "3 4 5 6 7 8 1234.5678"
                );
            }
        }
    }

    define_test!(return_i8, cdecl_func::return_i8, -1i8, ret_as_i8);
    define_test!(return_u8, cdecl_func::return_u8, 1u8, ret_as_u8);
    define_test!(
        return_isize,
        cdecl_func::return_isize,
        -1isize,
        ret_as_isize
    );
    define_test!(return_usize, cdecl_func::return_usize, 1usize, ret_as_usize);
    define_test!(return_i64, cdecl_func::return_i64, -1i64, ret_as_i64);
    define_test!(return_u64, cdecl_func::return_u64, 1u64, ret_as_u64);

    #[cfg(target_arch = "x86_64")]
    define_test!(return_i128, cdecl_func::return_i128, -1i128, ret_as_i128);
    #[cfg(target_arch = "x86_64")]
    define_test!(return_u128, cdecl_func::return_u128, 1u128, ret_as_u128);

    #[test]
    fn return_f32() {
        let mut func = Func::from_raw(cdecl_func::return_f32 as *const fn());
        func.push(123.456f32);
        unsafe {
            func.cdecl();
        }
        assert!(func.ret_as_f32() - 123.456 <= std::f32::EPSILON);
    }

    #[test]
    fn return_f64() {
        let mut func = Func::from_raw(cdecl_func::return_f64 as *const fn());
        func.push(123.456f64);
        unsafe {
            func.cdecl();
        }
        assert!(func.ret_as_f64() - 123.456 <= std::f64::EPSILON);
    }
}
