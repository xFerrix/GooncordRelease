fn main() {
    println!("cargo:warning=Running build.rs");
    embed_resource::compile("gooncord_icon.rc");
}
