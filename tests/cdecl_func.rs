pub extern "C" fn more_than_6_args(
    a: i32,
    b: i32,
    c: i32,
    d: i32,
    e: i32,
    f: i32,
    g: i32,
    h: i32,
) -> i32 {
    a + b + c + d + e + f + g + h
}

macro_rules! define_functions {
    ($cv:tt, $func:ident, $ty:ty) => {
        pub extern $cv fn $func(n: $ty) -> $ty {
            n
        }
    };
}

define_functions!("C", return_i8, i8);
define_functions!("C", return_u8, u8);
define_functions!("C", return_isize, isize);
define_functions!("C", return_usize, usize);
define_functions!("C", return_i64, i64);
define_functions!("C", return_u64, u64);
define_functions!("C", return_f32, f32);
define_functions!("C", return_f64, f64);

#[cfg(target_arch = "x86_64")]
define_functions!("C", return_i128, i128);

#[cfg(target_arch = "x86_64")]
define_functions!("C", return_u128, u128);
