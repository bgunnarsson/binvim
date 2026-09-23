// Compiles the vendored Markdown block grammar (vendor/tree-sitter-markdown,
// see its README), the same way tree-sitter-md's own build script did.
fn main() {
    let src = std::path::Path::new("vendor/tree-sitter-markdown/src");
    let mut build = cc::Build::new();
    build.std("c11").include(src);
    #[cfg(target_env = "msvc")]
    build.flag("-utf-8");
    for file in ["parser.c", "scanner.c"] {
        let path = src.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        build.file(path);
    }
    build.compile("tree-sitter-markdown");
}
