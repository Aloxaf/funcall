funcall
=======

根据指定调用约定动态调用函数

Examples
--------

```rust
use funcall::Func;

fn main() {
    let mut func = Func::new("/usr/lib/libc.so.6", b"printf\0");
    func.push(b"%d %f\0".as_ptr());
    func.push(2233);
    func.push(2233.3322);
    unsafe {
        func.cdecl();
    }
}
```