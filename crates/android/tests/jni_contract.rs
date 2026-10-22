//! The JNI boundary is stringly typed and only fails at runtime on a phone.
//! This checks both directions against the sources:
//!
//! * every `native` method in PatchNative.java has a Rust export with the
//!   same parameter types, and vice versa;
//! * every static method Rust calls exists in PatchNative.java with exactly
//!   the JNI signature Rust uses.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn java() -> String {
    read("../../android/app/src/main/java/com/mdhaarith/patch/PatchNative.java")
}

/// Java method declarations `static <ret> name(params)`, keyed by name.
fn java_methods(native: bool) -> BTreeMap<String, (String, Vec<String>)> {
    // Declarations may wrap; split into statements instead of lines.
    let src: String = java()
        .lines()
        .map(|l| l.split("//").next().unwrap().trim())
        .filter(|l| !l.starts_with('*') && !l.starts_with("/*") && !l.starts_with('@'))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = BTreeMap::new();
    for line in src.split(['{', ';', '}']).map(str::trim) {
        let Some(rest) = line.strip_prefix("static ") else {
            continue;
        };
        let (is_native, rest) = match rest.strip_prefix("native ") {
            Some(r) => (true, r),
            None => (false, rest),
        };
        if is_native != native || !rest.contains('(') || rest.contains('=') {
            continue;
        }
        let (head, params) = rest.split_once('(').unwrap();
        let mut head = head.split_whitespace();
        let (Some(ret), Some(name)) = (head.next(), head.next()) else {
            continue;
        };
        let params = params.split(')').next().unwrap();
        let types = params
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.split_whitespace().next().unwrap().to_string())
            .collect();
        out.insert(name.to_string(), (ret.to_string(), types));
    }
    out
}

fn jni_type(java: &str) -> &'static str {
    match java {
        "int" => "I",
        "float" => "F",
        "boolean" => "Z",
        "void" => "V",
        "String" => "Ljava/lang/String;",
        "Context" => "Landroid/content/Context;",
        "Surface" => "Landroid/view/Surface;",
        other => panic!("add a mapping for Java type {other}"),
    }
}

fn rust_type(java: &str) -> &'static str {
    match java {
        "int" => "jint",
        "float" => "jfloat",
        "boolean" => "jboolean",
        "String" => "JString",
        "Context" | "Surface" => "JObject",
        other => panic!("add a mapping for Java type {other}"),
    }
}

/// Rust exports `Java_com_mdhaarith_patch_PatchNative_<name>(env, class, params…)`.
fn rust_exports() -> BTreeMap<String, Vec<String>> {
    let src = read("src/android/mod.rs");
    let mut out = BTreeMap::new();
    let prefix = "fn Java_com_mdhaarith_patch_PatchNative_";
    for (i, _) in src.match_indices(prefix) {
        let rest = &src[i + prefix.len()..];
        let (name, rest) = rest.split_once('(').unwrap();
        let params = rest.split(')').next().unwrap();
        let types: Vec<String> = params
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.split(':').nth(1).unwrap().trim().to_string())
            .collect();
        assert!(
            types.len() >= 2 && types[0] == "JNIEnv" && types[1] == "JClass",
            "{name}: (JNIEnv, JClass, …)"
        );
        out.insert(name.to_string(), types[2..].to_vec());
    }
    out
}

#[test]
fn native_methods_match_rust_exports() {
    let java = java_methods(true);
    let rust = rust_exports();
    assert!(!java.is_empty() && !rust.is_empty());
    assert_eq!(
        java.keys().collect::<Vec<_>>(),
        rust.keys().collect::<Vec<_>>(),
        "native method names"
    );
    for (name, (ret, params)) in &java {
        assert_eq!(ret, "void", "{name}: Rust exports return nothing");
        let expected: Vec<&str> = params.iter().map(|p| rust_type(p)).collect();
        assert_eq!(rust[name], expected, "{name}: parameter types");
    }
    let src = read("src/android/mod.rs");
    assert_eq!(
        src.matches("#[no_mangle]").count(),
        rust.len(),
        "every export is #[no_mangle]"
    );
}

#[test]
fn rust_calls_match_java_statics() {
    let java = java_methods(false);
    let src = read("src/android/bridge.rs");
    // Calls look like `…(self.class, "name", "(sig)ret"` or `call_void("name", "(sig)V"`.
    let mut calls = Vec::new();
    for piece in src.split('"').collect::<Vec<_>>().windows(3) {
        let (name, sep, sig) = (piece[0], piece[1], piece[2]);
        if sig.starts_with('(')
            && sep.trim() == ","
            && name.chars().all(|c| c.is_ascii_alphanumeric())
            && !name.is_empty()
        {
            calls.push((name.to_string(), sig.to_string()));
        }
    }
    assert!(calls.len() >= 10, "found the bridge's calls: {calls:?}");
    for (name, sig) in calls {
        let (ret, params) = java
            .get(&name)
            .unwrap_or_else(|| panic!("PatchNative.{name} is missing"));
        let expected = format!(
            "({}){}",
            params.iter().map(|p| jni_type(p)).collect::<String>(),
            jni_type(ret)
        );
        assert_eq!(sig, expected, "PatchNative.{name} signature");
    }
}
