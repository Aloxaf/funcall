//! 动态调用函数 (目前仅支持小端序)

#![feature(asm)]

use std::ffi::OsStr;
use std::mem;

use num_traits::Num;
use std::any::{Any, TypeId};

type Result<T> = std::io::Result<T>;

/// cdecl 调用约定, C 函数默认使用的调用约定
///
/// TODO: 示例代码
///
/// # 传参方式
///
/// - 32位: 参数从右至左依次压栈, 不足 32 位的补齐 32 位, 超过 32 位的由高到低依次压栈
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Func {
    func: *const fn(),
    args: Vec<usize>,
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
            })
        }
    }

    /// 压入基本数字类型参数
    pub fn push<T: Num + Any>(&mut self, arg: T) {
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
            let slice = unsafe {
                std::slice::from_raw_parts(&arg as *const _ as *const usize, len)
            };
            self.args.extend_from_slice(slice);
        }
    }

    /// 压入指针类型的参数
    pub fn push_ref<T>(&mut self, arg: &T) {
        self.args.push(unsafe { mem::transmute_copy::<T, usize>(&arg) });
    }

    // TODO: 返回值
    #[cfg(target_arch = "x86")]
    pub unsafe fn cdecl_call(&self) {
        let mut low: usize;
        let mut high: usize;
        let mut double: f64 = 0.0; // TODO:
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
            : "={eax}"(low) "={edx}"(high) //"=f"(double)
            : "{eax}"(end_of_args) "{ebx}"(self.args.len()) "{ecx}"(self.func)
            : "eax" "ebx" "ecx" "edx"
            : "intel");
        println!("{} {} {}", low, high, double);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_arch = "x86")]
    fn cdecl() {
        let mut func = Func::new("/usr/lib32/libc.so.6", b"printf\0").unwrap();
        func.push_ref(&b"%d %lld %f %f\0");
        func.push(2233i32);
        func.push(2147483648i64);
        func.push(2233.0f32);
        func.push(123.456f64);
        println!("{:?}", func);
        unsafe {
            func.cdecl_call();
        }
    }
}
