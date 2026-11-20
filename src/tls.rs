//! 进程级 rustls provider（reqwest 用 rustls-no-provider 编译，必须先安装）。

pub fn ensure() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
