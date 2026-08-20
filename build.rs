fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/ramdog.ico");
        res.set("ProductName", "RamDog");
        res.set("FileDescription", "RamDog");
        res.compile().expect("embed ramdog.ico");
    }
}
