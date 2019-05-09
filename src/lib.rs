//! 动态调用函数 (目前仅支持小端序)
#![feature(proc_macro_hygiene, asm)]

use std::any::{Any, TypeId};
use std::ffi::OsStr;
use std::mem;

use rusty_asm::rusty_asm;

/// 可以被当成参数传递的类型
/// 包含裸指针和 primitive 类型
pub trait Arg {} // TODO: ToArg ?

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
    pub fn push<T: Arg + Any>(&mut self, arg: T) {
        // 64位下前八个浮点数需要用 xmm0~xmm7 传递
        if cfg!(target_arch = "x86_64") && self.fargs.len() != 8 {
            if arg.type_id() == TypeId::of::<f32>() {
                unsafe {
                    return self.fargs.push(f64::from(mem::transmute_copy::<T, f32>(&arg)));
                }
            } else if arg.type_id() == TypeId::of::<f64>() {
                unsafe {
                    return self.fargs.push(mem::transmute_copy::<T, f64>(&arg));
                }
            }
        }

        // 浮点数理应使用浮点专用的指令传参, 然而咱是手动压栈的, 压栈的时候已经丢失了类型信息, 只能用 push
        // 所以需要手动转成适合压栈的格式, 对于 f64 来说, 规则和 u64 一样, 但是 f32 需要先转成 f64 再压栈(即对齐
        if arg.type_id() == TypeId::of::<f32>() {
            unsafe {
                self.push(f64::from(mem::transmute_copy::<T, f32>(&arg)));
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
                    _ => unreachable!("We don't support 128bit machine now"),
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
        let mut double: f64;
        // 参数从右往左入栈, 因此先取得最右边的地址
        let end_of_args = self.args.as_ptr().offset(self.args.len() as isize - 1);
        // TODO: 此处备份到寄存器可能会失效?
        asm!(r#"
            mov  edi, ebx  // 备份参数个数

            dec  ebx       // 将 ecx 个参数依次压栈
            .LCDECL:
            push dword ptr [eax]
            sub  eax, 4
            dec  ebx
            jns  .LCDECL

            call ecx       // 调用函数

            shl  edi, 2    // 参数个数x4, 得到恢复堆栈指针所需的大小
            add  esp, edi  // 恢复堆栈指针
            "#
            : "={eax}"(low) "={edx}"(high) "={st}"(double) // https://github.com/rust-lang/rust/issues/20213
            : "{eax}"(end_of_args) "{ebx}"(self.args.len()) "{ecx}"(self.func)
            : "edi"
            : "intel");
        self.ret_low = low;
        self.ret_high = high;
        self.ret_float = double;
    }

    /// 64 位 Linux 默认使用的调用约定
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    pub unsafe fn cdecl(&mut self) {
        let mut low: usize;
        let mut high: usize;
        let mut double: f64;
        // 参数从右往左入栈, 因此先取得最右边的地址
        let end_of_args = self.args.as_ptr().offset(self.args.len() as isize - 1);
        let end_of_fargs = self.fargs.as_ptr().offset(self.fargs.len() as isize - 1);
        asm!(r#"
            // 前八个浮点参数传入浮点寄存器
            cmp rdx, 0
            jz FL0
            cmp rdx, 1
            jz FL1
            cmp rdx, 2
            jz FL2
            cmp rdx, 3
            jz FL3
            cmp rdx, 4
            jz FL4
            cmp rdx, 5
            jz FL5
            cmp rdx, 6
            jz FL6
            cmp rdx, 7
            jz FL7
            movsd xmm7, qword ptr [rcx]
            sub rcx, 8
            FL7:
            movsd xmm6, qword ptr [rcx]
            sub rcx, 8
            FL6:
            movsd xmm5, qword ptr [rcx]
            sub rcx, 8
            FL5:
            movsd xmm4, qword ptr [rcx]
            sub rcx, 8
            FL4:
            movsd xmm3, qword ptr [rcx]
            sub rcx, 8
            FL3:
            movsd xmm2, qword ptr [rcx]
            sub rcx, 8
            FL2:
            movsd xmm1, qword ptr [rcx]
            sub rcx, 8
            FL1:
            movsd xmm0, qword ptr [rcx]
            FL0:


            // 前六个整形参数入寄存器
            cmp rbx, 6
            jae L6      // if rbx >= 6, jmp L6
            cmp rbx, 5
            jz L5       // if rbx == 5, jmp L5
            cmp rbx, 4
            jz L4       // if rbx == 4, jmp L4
            cmp rbx, 3
            jz L3       // if rbx == 3, jmp L3
            cmp rbx, 2
            jz L2       // if rbx == 2, jmp L2
            jmp L1      // else jmp L1
            L6:
            dec rbx
            mov r9, qword ptr [rax]
            sub rax, 8
            L5:
            dec rbx
            mov r8, qword ptr [rax]
            sub rax, 8
            L4:
            dec rbx
            mov rcx, qword ptr [rax]
            sub rax, 8
            L3:
            dec rbx
            mov rdx, qword ptr [rax]
            sub rax, 8
            L2:
            dec rbx
            mov rsi, qword ptr [rax]
            sub rax, 8
            L1:
            dec rbx
            mov rdi, qword ptr [rax]

            mov r11, rbx  // 备份此时的 rbx, 以供清栈使用

            // 如果 r11 不为 0, 继续压剩下的参数
            cmp r11, 0
            jz CALL

            PUSH:
            push qword ptr [rax]
            sub rax, 8
            dec r11
            jns PUSH

            // 调用函数
            CALL:
            call r10

            // 如果 r11 不为 0, 则需要清栈
            cmp rbx, 0
            jz END
            shl rbx, 3
            add rsp, rbx

            END:
            "#
            : "={rax}"(low) "={rdx}"(high) "={xmm0}"(double) // https://github.com/rust-lang/rust/issues/20213
            : "{rax}"(end_of_args) "{rbx}"(self.args.len()) "{r10}"(self.func) "{rcx}"(end_of_fargs) "{rdx}"(self.fargs.len())
            : "rax" "rdx" "r10" "r11" "rdi" "rsi" "rdx" "rcx" "r8" "r9"
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
        let mut double: f64;
        // 参数从右往左入栈, 因此先取得最右边的地址
        let end_of_args = self.args.as_ptr().offset(self.args.len() as isize - 1);
        asm!(r#"
            dec ebx       // 将 ecx 个参数依次压栈
            LOOP_STDCALL:
            push dword ptr [eax]
            sub eax, 4
            dec ebx
            jns LOOP_STDCALL

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
    #[cfg(target_os = "linux")]
    fn cdecl_printf() {
        let mut buf = vec![0i8; 100];
        let mut func = if cfg!(target_arch = "x86") {
            Func::new("/usr/lib32/libc.so.6", b"sprintf\0").unwrap()
        } else {
            Func::new("/usr/lib/libc.so.6", b"sprintf\0").unwrap()
        };
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
