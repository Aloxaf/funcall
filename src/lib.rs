//! 动态调用函数 (目前仅支持小端序)

#![feature(asm)]

use std::any::{Any, TypeId};
use std::ffi::OsStr;
use std::mem;

/// 可以被当成参数传递的类型
/// 包含裸指针和 primitive 类型
pub trait Arg {}

impl<T> Arg for *const T {}

impl<T> Arg for *mut T {}

macro_rules! impl_arg {
    ($($ty:ty), *) => {
        $(impl Arg for $ty {})*
    };
}

impl_arg!(i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize, f32, f64);

type Result<T> = std::io::Result<T>;

/// # Example
///
/// ```
/// use apicall::Func;
///
/// let mut func = Func::new("/usr/lib32/libc.so.6", b"printf\0").unwrap();
/// func.push(b"%d".as_ptr());
/// func.push(2233);
/// unsafe {
///     func.cdecl();
/// }
/// ```
#[derive(Debug, Clone, PartialOrd, PartialEq)]
pub struct Func {
    func: *const fn(),
    args: Vec<usize>,
    ret_low: usize,
    ret_high: usize,
    ret_float: f64,
}

impl Func {
    /// 从 lib 中加载一个函数
    pub fn new<P: AsRef<OsStr>>(lib: P, func: &[u8]) -> Result<Self> {
        // TODO: 是否需要先尝试 dlopen / GetModuleHandle 来节省时间? (待确认
        let lib = libloading::Library::new(lib)?;
        unsafe {
            let func = lib.get::<fn()>(func)?;
            Ok(Self {
                func: *func.into_raw() as *const fn(),
                args: Vec::new(),
                ret_low: 0,
                ret_high: 0,
                ret_float: 0.0,
            })
        }
    }

    // TODO: 限制参数类型: primitive 类型或 裸指针
    /// 压入参数
    pub fn push<T: Arg + Any>(&mut self, arg: T) {
        // 浮点数理应使用浮点专用的指令传参, 然而咱是手动压栈的, 压栈的时候已经丢失了类型信息, 只能用 push
        // 所以需要手动转成适合压栈的格式, 对于 f64 来说, 规则和 u64 一样, 但是 f32 需要先转成 f64 再压栈(即对齐
        if arg.type_id() == TypeId::of::<f32>() {
            unsafe {
                self.push(mem::transmute_copy::<T, f32>(&arg) as f64);
            }
        } else if mem::size_of::<T>() <= mem::size_of::<usize>() {
            // 当参数大小小于等于机器字长时, 直接 transmute_copy + as 转换为 usize
            // 转换得到的数字需要与转换前的数字拥有相等的二进制表示(对齐后)
            let arg = unsafe {
                match mem::size_of::<T>() {
                    1 => mem::transmute_copy::<T, u8>(&arg) as usize,
                    2 => mem::transmute_copy::<T, u16>(&arg) as usize,
                    4 => mem::transmute_copy::<T, u32>(&arg) as usize,
                    8 => mem::transmute_copy::<T, u64>(&arg) as usize,
                    _ => unreachable!("128 位计算机???"),
                }
            };
            self.args.push(arg);
        } else {
            // 当参数大小大于机器字长, 如 32 位下的 f64 时
            // 分割为一个 &[usize] 再压入
            let len = mem::size_of::<T>() / mem::size_of::<usize>();
            let slice =
                unsafe { std::slice::from_raw_parts(&arg as *const _ as *const usize, len) };
            self.args.extend_from_slice(slice);
        }
    }

    /// 以 cdecl 调用约定调用函数
    /// 即 C 语言默认使用的调用约定
    #[cfg(target_arch = "x86")]
    pub unsafe fn cdecl(&mut self) {
        let mut low: usize;
        let mut high: usize;
        let mut double: f64; // TODO:
                             // 参数从右往左入栈, 因此先取得最右边的地址
        let end_of_args = self.args.as_ptr().offset(self.args.len() as isize - 1);
        asm!(r#"
            mov edi, ebx  // 备份参数个数

            dec ebx       // 将 ecx 个参数依次压栈
            LOOP:
            push dword ptr [eax]
            sub eax, 4
            dec ebx
            jns LOOP

            call ecx  // 调用函数

            shl edi, 2    // 参数个数x4, 得到恢复堆栈指针所需的大小
            add esp, edi  // 恢复堆栈指针
            "#
            : "={eax}"(low) "={edx}"(high) "={st}"(double) // https://github.com/rust-lang/rust/issues/20213
            : "{eax}"(end_of_args) "{ebx}"(self.args.len()) "{ecx}"(self.func)
            : "eax" "ebx" "ecx" "edx"
            : "intel");
        self.ret_low = low;
        self.ret_high = high;
        self.ret_float = double;
    }

    /// 以 stdcall 调用约定调用函数
    /// 即 WINAPI 使用的调用约定
    #[cfg(target_arch = "x86")]
    pub unsafe fn stdcall(&mut self) {
        let mut low: usize;
        let mut high: usize;
        let mut double: f64; // TODO:
        // 参数从右往左入栈, 因此先取得最右边的地址
        let end_of_args = self.args.as_ptr().offset(self.args.len() as isize - 1);
        asm!(r#"
            mov edi, ebx  // 备份参数个数

            dec ebx       // 将 ecx 个参数依次压栈
            LOOP:
            push dword ptr [eax]
            sub eax, 4
            dec ebx
            jns LOOP

            call ecx  // 调用函数
            "#
            : "={eax}"(low) "={edx}"(high) "={st}"(double) // https://github.com/rust-lang/rust/issues/20213
            : "{eax}"(end_of_args) "{ebx}"(self.args.len()) "{ecx}"(self.func)
            : "eax" "ebx" "ecx" "edx"
            : "intel");
        self.ret_low = low;
        self.ret_high = high;
        self.ret_float = double;
    }

    pub fn ret_f64(&self) -> f64 {
        self.ret_float
    }

    pub fn ret_usize(&self) -> usize {
        self.ret_low
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    #[cfg(target_arch = "x86")]
    fn cdecl() {
        let mut buf = vec![0i8; 100];
        let mut func = Func::new("/usr/lib32/libc.so.6", b"sprintf\0").unwrap();
        func.push(buf.as_mut_ptr());
        func.push(b"%d %lld %.3f %.3f\0".as_ptr());
        func.push(2233i32);
        func.push(2147483648i64);
        func.push(2233.0f32);
        func.push(123.456f64);
        unsafe {
            func.cdecl();
            assert_eq!(
                CStr::from_ptr(buf.as_ptr()).to_str().unwrap(),
                "2233 2147483648 2233.000 123.456"
            );
        }

        let mut func = Func::new("/usr/lib32/libc.so.6", b"atoi\0").unwrap();
        func.push(b"2233\0".as_ptr());
        unsafe {
            func.cdecl();
        }
        assert_eq!(func.ret_usize(), 2233);

        let mut func = Func::new("/usr/lib32/libc.so.6", b"atof\0").unwrap();
        func.push(b"123.456\0".as_ptr());
        unsafe {
            func.cdecl();
        }
        assert!(func.ret_f64() - 123.456 <= std::f64::EPSILON);
    }
}
