//! 动态调用函数 (目前仅支持小端序)
#![feature(proc_macro_hygiene, asm)]

use std::any::{Any, TypeId};
use std::ffi::OsStr;
use std::mem;

use rusty_asm::rusty_asm;

/// 可以被当成参数传递的类型
/// 包含裸指针和 primitive 类型
pub trait ToArg {} // TODO: 应有 to_arg 方法

impl<T> ToArg for *const T {}

impl<T> ToArg for *mut T {}

macro_rules! impl_arg {
    ($($ty:ty), *) => {
        $(impl ToArg for $ty {})*
    };
}

impl_arg!(i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize, f32, f64);

type Result<T> = std::io::Result<T>;

/// # Example
///
/// ```ignore
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
    /// 被调用函数指针
    func: *const fn(),
    /// 32位下储存所有参数, 64位下储存所有整数参数与除前八个外的浮点参数
    args: Vec<usize>,
    /// 64位下储存前八个浮点参数
    fargs: Vec<f64>,
    /// 返回值低位
    ret_low: usize,
    /// 返回值高位
    ret_high: usize,
    /// 浮点寄存器的值
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
                fargs: Vec::new(),
                ret_low: 0,
                ret_high: 0,
                ret_float: 0.0,
            })
        }
    }

    /// 根据函数指针创建一个实例
    pub fn from_raw(ptr: *const fn()) -> Self {
        Self {
            func: ptr,
            args: Vec::new(),
            fargs: Vec::new(),
            ret_low: 0,
            ret_high: 0,
            ret_float: 0.0,
        }
    }

    /// 压入参数
    pub fn push<T: ToArg + Any>(&mut self, arg: T) {
        unsafe {
            // 64位下前八个浮点数需要用 xmm0~xmm7 传递
            if cfg!(target_arch = "x86_64") && self.fargs.len() != 8 {
                if arg.type_id() == TypeId::of::<f32>() {
                    return self
                        .fargs
                        .push(f64::from(mem::transmute_copy::<T, f32>(&arg)));
                } else if arg.type_id() == TypeId::of::<f64>() {
                    return self.fargs.push(mem::transmute_copy::<T, f64>(&arg));
                }
            }

            // 浮点数理应使用浮点专用的指令传参, 然而咱是手动压栈的, 压栈的时候已经丢失了类型信息, 只能用 push
            // 所以需要手动转成适合压栈的格式, 对于 f64 来说, 规则和 u64 一样, 但是 f32 需要先转成 f64 再压栈(即对齐
            if arg.type_id() == TypeId::of::<f32>() {
                self.push(f64::from(mem::transmute_copy::<T, f32>(&arg)));
            } else if mem::size_of::<T>() <= mem::size_of::<usize>() {
                // 当参数大小小于等于机器字长时, 直接 transmute_copy + as 转换为 usize
                // 转换得到的数字需要与转换前的数字拥有相等的二进制表示(对齐后)
                let arg = match mem::size_of::<T>() {
                    1 => mem::transmute_copy::<T, u8>(&arg) as usize,
                    2 => mem::transmute_copy::<T, u16>(&arg) as usize,
                    4 => mem::transmute_copy::<T, u32>(&arg) as usize,
                    8 => mem::transmute_copy::<T, u64>(&arg) as usize,
                    _ => unreachable!("We don't support 128bit machine now"),
                };
                self.args.push(arg);
            } else {
                // 当参数大小大于机器字长, 如 32 位下的 f64 时
                // 分割为一个 &[usize] 再压入
                let len = mem::size_of::<T>() / mem::size_of::<usize>();
                let slice = std::slice::from_raw_parts(&arg as *const _ as *const usize, len);
                self.args.extend_from_slice(slice);
            }
        }
    }

    /// 以 cdecl 调用约定调用函数
    /// 即 C 语言默认使用的调用约定
    #[cfg(target_arch = "x86")]
    pub unsafe fn cdecl(&mut self) {
        rusty_asm! {
            let mut low  : usize: out("{eax}");
            let mut high : usize: out("{edx}");
            let mut float: f64  : out("{st}");
            // 参数从右往左入栈, 因此先取得最右边的地址
            let args: in("r") = self.args.as_ptr().offset(self.args.len() as isize - 1);
            let len : in("m") = self.args.len();
            let func: in("m") = self.func;

            clobber("memory");
            clobber("esp");
            clobber("ebx");

            asm("intel") {r"
                mov  ebx, $len  // 将 $4 个参数依次压栈
                dec  ebx
            .L${:uid}:          // https://github.com/rust-lang/rust/issues/27395
                push dword ptr [$args]
                sub  $args, 4
                dec  ebx
                jns  .L${:uid}

                call $func      // 调用函数

                mov  ebx, $len
                shl  ebx, 2     // 参数个数x4, 得到恢复堆栈指针所需的大小
                add  esp, ebx   // 恢复堆栈指针
            "}

            self.ret_low   = low;
            self.ret_high  = high;
            self.ret_float = float;
        }
    }

    /// 64 位 Linux 默认使用的调用约定
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    pub unsafe fn cdecl(&mut self) {
        rusty_asm! {
            let mut low  : usize: out("{rax}");
            let mut high : usize: out("{rdx}");
            let mut float: f64  : out("{xmm0}"); // https://github.com/rust-lang/rust/issues/20213

            let args : in("r") = self.args.as_ptr().offset(self.args.len() as isize - 1);
            let len  : in("r") = self.args.len();
            let fargs: in("r") = self.fargs.as_ptr().offset(self.fargs.len() as isize - 1);
            let flen : in("r") = self.fargs.len();
            let func : in("m") = self.func;

            clobber("memory");
            clobber("rsp");
            clobber("rdi");
            clobber("rsi");
            clobber("rdx");
            clobber("rcx");
            clobber("r8");
            clobber("r9");
            clobber("r10"); // 调用者保护
            clobber("r11"); // 调用者保护
            clobber("r12");

            asm("alignstack", "intel") {r"
                // 需要送入寄存器的浮点参数个数一定不大于 8, 因此直接查表跳转即可
                lea    rdi, [rip + .LFLABELS${:uid}]
                movsxd rsi, dword ptr [rdi + $flen * 4]
                add    rsi, rdi
                jmp    rsi

            .LFLABELS${:uid}:
                .long .LARG0${:uid}-.LFLABELS${:uid}
                .long .LARG1${:uid}-.LFLABELS${:uid}
                .long .LARG2${:uid}-.LFLABELS${:uid}
                .long .LARG3${:uid}-.LFLABELS${:uid}
                .long .LARG4${:uid}-.LFLABELS${:uid}
                .long .LARG5${:uid}-.LFLABELS${:uid}
                .long .LARG6${:uid}-.LFLABELS${:uid}
                .long .LARG7${:uid}-.LFLABELS${:uid}
                .long .LARG8${:uid}-.LFLABELS${:uid}

            .LARG8${:uid}:
                movsd xmm7, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG7${:uid}:
                movsd xmm6, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG6${:uid}:
                movsd xmm5, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG5${:uid}:
                movsd xmm4, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG4${:uid}:
                movsd xmm3, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG3${:uid}:
                movsd xmm2, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG2${:uid}:
                movsd xmm1, qword ptr [$fargs]
                sub   $fargs, 8
            .LARG1${:uid}:
                movsd xmm0, qword ptr [$fargs]
            .LARG0${:uid}:


                xor    r12, r12    // r12 记录压栈参数数目
            .LPUSH${:uid}:         // 将参数压栈, 直到参数个数小于等于 6
                cmp    $len, 6
                jbe    .LPUSH_F6${:uid}
                push   qword ptr [$args]
                sub    $args, 8
                inc    r12
                dec    $len
                jmp    .LPUSH${:uid}

            .LPUSH_F6${:uid}:    // 将前六个参数送入寄存器
                lea    rdi, [rip + .LABELS${:uid}]
                movsxd rsi, dword ptr [rdi + $len * 4]
                add    rsi, rdi
                jmp    rsi

            .LABELS${:uid}:
                .long .LCALL${:uid}-.LABELS${:uid}
                .long .L1${:uid}-.LABELS${:uid}
                .long .L2${:uid}-.LABELS${:uid}
                .long .L3${:uid}-.LABELS${:uid}
                .long .L4${:uid}-.LABELS${:uid}
                .long .L5${:uid}-.LABELS${:uid}
                .long .L6${:uid}-.LABELS${:uid}

            .L6${:uid}:
                mov  r9, qword ptr [$args]
                sub  $args, 8
            .L5${:uid}:
                mov  r8, qword ptr [$args]
                sub  $args, 8
            .L4${:uid}:
                mov  rcx, qword ptr [$args]
                sub  $args, 8
            .L3${:uid}:
                mov  rdx, qword ptr [$args]
                sub  $args, 8
            .L2${:uid}:
                mov  rsi, qword ptr [$args]
                sub  $args, 8
            .L1${:uid}:
                mov  rdi, qword ptr [$args]

            .LCALL${:uid}:
                call $func

                // 如果 r12 不为 0, 则需要清栈
                cmp  r12, 0
                jz   .LEND${:uid}
                shl  r12, 3
                add  rsp, r12

            .LEND${:uid}:
            "}

            self.ret_low   = low;
            self.ret_high  = high;
            self.ret_float = float;
        }
    }

    /// 以 stdcall 调用约定调用函数
    /// 即 WINAPI 使用的调用约定
    #[cfg(target_arch = "x86")]
    pub unsafe fn stdcall(&mut self) {
        rusty_asm! {
            let mut low  : usize: out("{eax}");
            let mut high : usize: out("{edx}");
            let mut float: f64  : out("{st}");
            // 参数从右往左入栈, 因此先取得最右边的地址
            let args: in("r") = self.args.as_ptr().offset(self.args.len() as isize - 1);
            let len : in("m") = self.args.len();
            let func: in("m") = self.func;

            clobber("memory");
            clobber("esp");
            clobber("ebx");

            asm("intel") {r"
                mov  ebx, $len  // 将 $4 个参数依次压栈
                dec  ebx
            .L${:uid}:          // https://github.com/rust-lang/rust/issues/27395
                push dword ptr [$args]
                sub  $args, 4
                dec  ebx
                jns  .L${:uid}

                call $func      // 调用函数
            "}

            self.ret_low   = low;
            self.ret_high  = high;
            self.ret_float = float;
        }
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

    #[test]
    fn cdecl_more_than_6_args() {
        let mut func = Func::from_raw(more_than_6_args as *const fn());
        for i in 1..=8 {
            func.push(i);
        }
        unsafe {
            func.cdecl();
            assert_eq!(func.ret_usize(), (1..=8).sum());
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cdecl_sprintf() {
        let mut buf = vec![0i8; 100];
        let mut func = if cfg!(target_arch = "x86") {
            Func::new("/usr/lib32/libc.so.6", b"sprintf\0").unwrap()
        } else {
            Func::new("/usr/lib/libc.so.6", b"sprintf\0").unwrap()
        };
        func.push(buf.as_mut_ptr());
        func.push(b"%d %d %d %d %d %d\0".as_ptr());
        func.push(3);
        func.push(4);
        func.push(5);
        func.push(6);
        func.push(7);
        func.push(8);
        unsafe {
            func.cdecl();
            assert_eq!(
                CStr::from_ptr(buf.as_ptr()).to_str().unwrap(),
                "3 4 5 6 7 8"
            );
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cdecl_return_int() {
        let mut func = if cfg!(target_arch = "x86") {
            Func::new("/usr/lib32/libc.so.6", b"atoi\0").unwrap()
        } else {
            Func::new("/usr/lib/libc.so.6", b"atoi\0").unwrap()
        };
        func.push(b"2233\0".as_ptr());
        unsafe {
            func.cdecl();
        }
        assert_eq!(func.ret_usize(), 2233);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cdecl_return_long_long() {
        let mut func = if cfg!(target_arch = "x86") {
            Func::new("/usr/lib32/libc.so.6", b"atoll\0").unwrap()
        } else {
            Func::new("/usr/lib/libc.so.6", b"atoll\0").unwrap()
        };
        func.push(b"2147483649\0".as_ptr());
        unsafe {
            func.cdecl();
        }
        assert_eq!(func.ret_usize(), 2147483649);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cdecl_return_double() {
        let mut func = if cfg!(target_arch = "x86") {
            Func::new("/usr/lib32/libc.so.6", b"atof\0").unwrap()
        } else {
            Func::new("/usr/lib/libc.so.6", b"atof\0").unwrap()
        };
        func.push(b"123.456\0".as_ptr());
        unsafe {
            func.cdecl();
        }
        assert!(func.ret_f64() - 123.456 <= std::f64::EPSILON);
    }
}
