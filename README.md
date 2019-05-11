(WIP)funcall
=======

根据指定调用约定动态调用函数

示例
------

```rust
use funcall::Func;
extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}

let mut func = Func::from_raw(add as *const fn());
func.push(1i32);
func.push(1i32);
unsafe {
    func.cdecl();
}

assert_eq!(func.ret_as_i32(), 2);
```


```rust
use funcall::Func;
use std::ffi::CStr;

let mut func = Func::new("/usr/lib/libc.so.6", b"sprintf\0").unwrap();
let mut buf = vec![0i8; 100];
func.push(buf.as_mut_ptr());
func.push(b"%d %.6f\0".as_ptr());
func.push(2233i32);
func.push(2233.3322f64);
unsafe {
    func.cdecl();
    assert_eq!(CStr::from_ptr(buf.as_ptr()).to_str().unwrap(), "2233 2233.332200")
}
```