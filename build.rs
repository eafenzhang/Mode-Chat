fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icons/app.ico");
    #[cfg(windows)]
    {
        // 把 K1 应用图标打进 exe：资源管理器/快捷方式/安装器都会显示。
        let _ = embed_resource::compile("assets/app.rc", embed_resource::NONE);
    }
}
