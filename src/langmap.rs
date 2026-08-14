//! Extension → Prism language, pure data.
//! Ported from MoremaidApp Sources/Rendering/LanguageMaps.swift @ a3ab7fd.
//! Only the file-level maps survive — fence-info aliases live in page.js, and
//! the dependency-chain script-tag machinery is obsolete: the vendored Prism
//! autoloader resolves grammars (and their dependencies) at render time.

const EXTENSION_TO_LANGUAGE: &[(&str, &str)] = &[
    // Web
    ("js", "javascript"), ("mjs", "javascript"), ("cjs", "javascript"),
    ("ts", "typescript"), ("jsx", "jsx"), ("tsx", "tsx"),
    ("html", "markup"), ("htm", "markup"), ("xml", "markup"), ("svg", "markup"),
    ("css", "css"), ("scss", "scss"), ("sass", "sass"), ("less", "less"),
    ("json", "json"), ("jsonc", "json"),
    ("graphql", "graphql"), ("gql", "graphql"),
    ("vue", "markup"), ("svelte", "markup"),
    // Config
    ("toml", "toml"), ("yaml", "yaml"), ("yml", "yaml"),
    ("ini", "ini"), ("cfg", "ini"), ("conf", "ini"),
    ("properties", "properties"), ("env", "bash"),
    ("hcl", "hcl"), ("tf", "hcl"), ("tfvars", "hcl"), ("nginx", "nginx"),
    // Shell & scripting
    ("sh", "bash"), ("bash", "bash"), ("zsh", "bash"), ("fish", "bash"),
    ("bat", "batch"), ("cmd", "batch"), ("ps1", "powershell"), ("psm1", "powershell"),
    // Systems
    ("c", "c"), ("h", "c"),
    ("cpp", "cpp"), ("cxx", "cpp"), ("cc", "cpp"), ("hpp", "cpp"), ("hxx", "cpp"),
    ("rs", "rust"), ("go", "go"), ("zig", "zig"),
    // JVM
    ("java", "java"), ("kt", "kotlin"), ("kts", "kotlin"),
    ("scala", "scala"), ("groovy", "groovy"), ("gradle", "groovy"),
    // .NET
    ("cs", "csharp"), ("fs", "fsharp"), ("fsx", "fsharp"), ("vb", "visual-basic"),
    // Apple / mobile
    ("swift", "swift"), ("m", "objectivec"), ("mm", "objectivec"), ("dart", "dart"),
    // Scripting
    ("py", "python"), ("pyw", "python"), ("rb", "ruby"), ("php", "php"),
    ("pl", "perl"), ("pm", "perl"), ("lua", "lua"), ("r", "r"), ("jl", "julia"),
    // Functional
    ("ex", "elixir"), ("exs", "elixir"), ("erl", "erlang"),
    ("clj", "clojure"), ("cljs", "clojure"), ("hs", "haskell"),
    ("ml", "ocaml"), ("mli", "ocaml"), ("elm", "elm"),
    ("lisp", "lisp"), ("scm", "scheme"), ("rkt", "scheme"),
    // Data & query
    ("sql", "sql"), ("proto", "protobuf"),
    // Markup & docs
    // md/markdown only reach the code page via the view-source toggle
    ("md", "markdown"), ("markdown", "markdown"),
    ("tex", "latex"), ("latex", "latex"), ("rst", "rest"), ("adoc", "asciidoc"),
    ("pug", "pug"), ("handlebars", "handlebars"), ("hbs", "handlebars"), ("ejs", "ejs"),
    // DevOps & build
    ("dockerfile", "docker"), ("makefile", "makefile"), ("cmake", "cmake"),
    // Diff & patch
    ("diff", "diff"), ("patch", "diff"),
    // Misc
    ("vim", "vim"), ("regex", "regex"), ("wasm", "wasm"),
    ("txt", "plaintext"), ("log", "plaintext"),
];

const FILENAME_TO_LANGUAGE: &[(&str, &str)] = &[
    ("Dockerfile", "docker"),
    ("Makefile", "makefile"),
    ("Gemfile", "ruby"),
    ("Rakefile", "ruby"),
    ("CMakeLists.txt", "cmake"),
    ("Vagrantfile", "ruby"),
    ("Justfile", "makefile"),
    (".gitignore", "git"),
    (".gitattributes", "git"),
    (".editorconfig", "editorconfig"),
    (".dockerignore", "docker"),
    (".bashrc", "bash"),
    (".bash_profile", "bash"),
    (".zshrc", "bash"),
    (".profile", "bash"),
];

/// Resolve the Prism language for a filename.
pub fn language_for_file(file_name: &str) -> &'static str {
    let basename = file_name.rsplit('/').next().unwrap_or(file_name);
    if let Some((_, lang)) = FILENAME_TO_LANGUAGE.iter().find(|(n, _)| *n == basename) {
        return lang;
    }
    let ext = match basename.rsplit_once('.') {
        // ".gitignore"-style names have no extension, only a leading dot
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => return "plaintext",
    };
    EXTENSION_TO_LANGUAGE
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, lang)| *lang)
        .unwrap_or("plaintext")
}

pub fn is_markdown(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// HTML files render as documents, not as highlighted markup (§7 htmlPage).
pub fn is_html(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    lower.ends_with(".html") || lower.ends_with(".htm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(language_for_file("main.rs"), "rust");
        assert_eq!(language_for_file("a/b/script.PY"), "python");
        assert_eq!(language_for_file("notes.weird"), "plaintext");
    }

    #[test]
    fn special_filenames() {
        assert_eq!(language_for_file("Dockerfile"), "docker");
        assert_eq!(language_for_file("src/Makefile"), "makefile");
        assert_eq!(language_for_file(".zshrc"), "bash");
        assert_eq!(language_for_file("README"), "plaintext");
    }

    #[test]
    fn markdown_detection() {
        assert!(is_markdown("README.md"));
        assert!(is_markdown("notes.MARKDOWN"));
        assert!(!is_markdown("main.rs"));
    }

    #[test]
    fn html_detection() {
        assert!(is_html("page.html"));
        assert!(is_html("INDEX.HTM"));
        assert!(!is_html("page.html.bak"));
        assert!(!is_html("README.md"));
    }
}
