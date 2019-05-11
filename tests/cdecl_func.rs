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

pub extern "C" fn return_i8(n: i8) -> i8 {
    n
}

pub extern "C" fn return_u8(n: u8) -> u8 {
    n
}

pub extern "C" fn return_isize(n: isize) -> isize {
    n
}

pub extern "C" fn return_usize(n: usize) -> usize {
    n
}

pub extern "C" fn return_i64(n: i64) -> i64 {
    n
}

pub extern "C" fn return_u64(n: u64) -> u64 {
    n
}

#[cfg(target_arch = "x86_64")]
pub extern "C" fn return_i128(n: i128) -> i128 {
    n
}

#[cfg(target_arch = "x86_64")]
pub extern "C" fn return_u128(n: u128) -> u128 {
    n
}

pub extern "C" fn return_f32(n: f32) -> f32 {
    n
}

pub extern "C" fn return_f64(n: f64) -> f64 {
    n
}
