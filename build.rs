fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/ramdog.ico");
        res.set("ProductName", "RamDog");
        res.set("FileDescription", "RamDog");
        res.compile().expect("embed ramdog.ico");
    }
}
