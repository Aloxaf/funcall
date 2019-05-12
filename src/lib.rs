//! 根据指定调用约定动态调用函数 (目前仅支持小端序)
//!
//! # 示例
//!
//! ```
//! use funcall::Func;
//! extern "C" fn add(a: i32, b: i32) -> i32 {
//!     a + b
//! }
//!
//! let mut func = Func::from_raw(add as *const fn());
//! func.push(1i32);
//! func.push(1i32);
//! unsafe {
//!     func.cdecl();
//! }
//!
//! assert_eq!(func.ret_as_i32(), 2);
//! ```
//!
//! ```
//! use funcall::Func;
//! use std::ffi::CStr;
//!
//! let mut func = Func::new("/usr/lib/libc.so.6", b"sprintf\0").unwrap();
//! let mut buf = vec![0i8; 100];
//! func.push(buf.as_mut_ptr());
//! func.push(b"%d %.6f\0".as_ptr());
//! func.push(2233i32);
//! func.push(2233.3322f64);
//! unsafe {
//!     func.cdecl();
//!     assert_eq!(CStr::from_ptr(buf.as_ptr()).to_str().unwrap(), "2233 2233.332200")
//! }
//!
//! ```
#![feature(proc_macro_hygiene, asm)]

use std::any::{Any, TypeId};
use std::ffi::OsStr;
use std::mem;

use rusty_asm::rusty_asm;

/// 将参数转换为 Vec<usize> 方便压栈
pub trait IntoArg {
    fn into_arg(self) -> Vec<usize>;
}

impl<T> IntoArg for *const T {
    fn into_arg(self) -> Vec<usize> {
        vec![self as usize]
    }
}

impl<T> IntoArg for *mut T {
    fn into_arg(self) -> Vec<usize> {
        vec![self as usize]
    }
}

// f32 无论 32 位 还是 64 位下都要对齐到 64 位再传参
impl IntoArg for f32 {
    fn into_arg(self) -> Vec<usize> {
        (self as f64).into_arg()
    }
}

macro_rules! impl_intoarg {
    ($($ty:ty), *) => {
        $(impl IntoArg for $ty {
            fn into_arg(self) -> Vec<usize> {
                let len = mem::size_of::<$ty>() / mem::size_of::<usize>();
                if len <= 1 {
                    // 小于等于机器字长的参数, 直接对齐就行了
                    vec![self as usize]
                } else {
                    // 大于机器字长的参数, 分割为 Vec<usize>
                    unsafe {
                        std::slice::from_raw_parts(&self as *const _ as *const usize, len).to_vec()
                    }
                }
            }
        })*
    };
}

impl_intoarg!(i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize, f64);

type Result<T> = std::io::Result<T>;

/// # 示例
///
/// ```ignore
/// use funcall::Func;
///
/// let mut func = Func::new("/usr/lib/libc.so.6", b"printf\0").unwrap();
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
    /// 从 lib 中加载一个函数, 注意 func 需要以 '\0' 结尾
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
    pub fn push<T: IntoArg + Any>(&mut self, arg: T) {
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
            self.args.extend_from_slice(&arg.into_arg());
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
            let args: in("r") = self.args.as_ptr().wrapping_offset(self.args.len() as isize - 1);
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

                mov  ebx, $len  // 恢复堆栈指针
                lea  esp, [esp + ebx * 4]
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

            let args : in("r") = self.args.as_ptr().wrapping_offset(self.args.len() as isize - 1);
            let len  : in("r") = self.args.len();
            let fargs: in("r") = self.fargs.as_ptr().wrapping_offset(self.fargs.len() as isize - 1);
            let flen : in("r") = self.fargs.len();
            let func : in("m") = self.func;

            clobber("memory");
            clobber("rsp");

            clobber("rdi"); // 传参寄存器
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

                // r12 = $len <= 6 ? 0 : ($len - 6)
                lea    r12, [$len - 6]
                cmp    $len, 6
                mov    rdi, 0
                cmovbe r12, rdi
                jbe    .LPUSH_F6${:uid}
            .LPUSH${:uid}:       // 将参数压栈, 直到参数个数小于等于 6
                push   qword ptr [$args]
                sub    $args, 8
                sub    $len, 1
                cmp    $len, 6   // if $len != 6
                jne    .LPUSH${:uid}

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

                // 清理堆栈
                lea  rsp, [rsp + r12 * 8]
            "}

            self.ret_low   = low;
            self.ret_high  = high;
            self.ret_float = float;
        }
    }

    /// 以 stdcall 调用约定调用函数
    /// 即 32 位下 WINAPI 使用的调用约定
    #[cfg(target_arch = "x86")]
    pub unsafe fn stdcall(&mut self) {
        rusty_asm! {
            let mut low  : usize: out("{eax}");
            let mut high : usize: out("{edx}");
            let mut float: f64  : out("{st}");
            // 参数从右往左入栈, 因此先取得最右边的地址
            let args: in("r") = self.args.as_ptr().wrapping_offset(self.args.len() as isize - 1);
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
}

impl Func {
    pub fn ret_as_i8(&self) -> i8 {
        self.ret_low as i8
    }

    pub fn ret_as_u8(&self) -> u8 {
        self.ret_low as u8
    }

    pub fn ret_as_i16(&self) -> i16 {
        self.ret_low as i16
    }

    pub fn ret_as_u16(&self) -> u16 {
        self.ret_low as u16
    }

    pub fn ret_as_i32(&self) -> i32 {
        self.ret_low as i32
    }

    pub fn ret_as_u32(&self) -> u32 {
        self.ret_low as u32
    }

    pub fn ret_as_i64(&self) -> i64 {
        self.ret_as_u64() as i64
    }

    pub fn ret_as_u64(&self) -> u64 {
        if cfg!(target_arch = "x86") {
            (self.ret_high as u64) << 32 | self.ret_low as u64
        } else {
            self.ret_low as u64
        }
    }

    pub fn ret_as_isize(&self) -> isize {
        self.ret_low as isize
    }

    pub fn ret_as_usize(&self) -> usize {
        self.ret_low as usize
    }

    pub fn ret_as_i128(&self) -> i128 {
        self.ret_as_u128() as i128
    }

    pub fn ret_as_u128(&self) -> u128 {
        if cfg!(target_arch = "x86_64") {
            (self.ret_high as u128) << 64 | self.ret_low as u128
        } else {
            unimplemented!()
        }
    }

    pub fn ret_as_f32(&self) -> f32 {
        self.ret_float as f32
    }

    pub fn ret_as_f64(&self) -> f64 {
        self.ret_float
    }
}
