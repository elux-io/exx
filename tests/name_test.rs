use exx::name::Name;

#[test]
fn names_are_interned() {
    assert_eq!(Name::from("toto"), Name::from("toto"));
    assert_ne!(Name::from("toto"), Name::from("titi"));
}

#[test]
fn as_str() {
    assert_eq!(Name::from("blabla").as_str(), "blabla");
}

#[test]
fn keywords() {
    assert!(Name::from("alignas").is_kw());
    assert!(Name::from("alignof").is_kw());
    assert!(Name::from("asm").is_kw());
    assert!(Name::from("auto").is_kw());
    assert!(Name::from("bool").is_kw());
    assert!(Name::from("break").is_kw());
    assert!(Name::from("case").is_kw());
    assert!(Name::from("catch").is_kw());
    assert!(Name::from("char").is_kw());
    assert!(Name::from("char16_t").is_kw());
    assert!(Name::from("char32_t").is_kw());
    assert!(Name::from("char8_t").is_kw());
    assert!(Name::from("class").is_kw());
    assert!(Name::from("co_await").is_kw());
    assert!(Name::from("co_return").is_kw());
    assert!(Name::from("co_yield").is_kw());
    assert!(Name::from("concept").is_kw());
    assert!(Name::from("const_cast").is_kw());
    assert!(Name::from("const").is_kw());
    assert!(Name::from("consteval").is_kw());
    assert!(Name::from("constexpr").is_kw());
    assert!(Name::from("constinit").is_kw());
    assert!(Name::from("continue").is_kw());
    assert!(Name::from("contract_assert").is_kw());
    assert!(Name::from("decltype").is_kw());
    assert!(Name::from("default").is_kw());
    assert!(Name::from("delete").is_kw());
    assert!(Name::from("do").is_kw());
    assert!(Name::from("double").is_kw());
    assert!(Name::from("dynamic_cast").is_kw());
    assert!(Name::from("else").is_kw());
    assert!(Name::from("enum").is_kw());
    assert!(Name::from("explicit").is_kw());
    assert!(Name::from("export").is_kw());
    assert!(Name::from("extern").is_kw());
    assert!(Name::from("false").is_kw());
    assert!(Name::from("float").is_kw());
    assert!(Name::from("for").is_kw());
    assert!(Name::from("friend").is_kw());
    assert!(Name::from("goto").is_kw());
    assert!(Name::from("if").is_kw());
    assert!(Name::from("inline").is_kw());
    assert!(Name::from("int").is_kw());
    assert!(Name::from("long").is_kw());
    assert!(Name::from("mutable").is_kw());
    assert!(Name::from("namespace").is_kw());
    assert!(Name::from("new").is_kw());
    assert!(Name::from("noexcept").is_kw());
    assert!(Name::from("nullptr").is_kw());
    assert!(Name::from("operator").is_kw());
    assert!(Name::from("private").is_kw());
    assert!(Name::from("protected").is_kw());
    assert!(Name::from("public").is_kw());
    assert!(Name::from("register").is_kw());
    assert!(Name::from("reinterpret_cast").is_kw());
    assert!(Name::from("requires").is_kw());
    assert!(Name::from("return").is_kw());
    assert!(Name::from("short").is_kw());
    assert!(Name::from("signed").is_kw());
    assert!(Name::from("sizeof").is_kw());
    assert!(Name::from("static_assert").is_kw());
    assert!(Name::from("static_cast").is_kw());
    assert!(Name::from("static").is_kw());
    assert!(Name::from("struct").is_kw());
    assert!(Name::from("switch").is_kw());
    assert!(Name::from("template").is_kw());
    assert!(Name::from("this").is_kw());
    assert!(Name::from("thread_local").is_kw());
    assert!(Name::from("throw").is_kw());
    assert!(Name::from("true").is_kw());
    assert!(Name::from("try").is_kw());
    assert!(Name::from("typedef").is_kw());
    assert!(Name::from("typeid").is_kw());
    assert!(Name::from("typename").is_kw());
    assert!(Name::from("union").is_kw());
    assert!(Name::from("unsigned").is_kw());
    assert!(Name::from("using").is_kw());
    assert!(Name::from("virtual").is_kw());
    assert!(Name::from("void").is_kw());
    assert!(Name::from("volatile").is_kw());
    assert!(Name::from("wchar_t").is_kw());
    assert!(Name::from("while").is_kw());

    // "identifiers with special meaning"
    assert!(Name::from("final").is_ctxt_kw());
    assert!(Name::from("import").is_ctxt_kw());
    assert!(Name::from("module").is_ctxt_kw());
    assert!(Name::from("override").is_ctxt_kw());
    assert!(Name::from("pre").is_ctxt_kw());
    assert!(Name::from("post").is_ctxt_kw());

    // attributes
    assert!(Name::from("assume").is_attr_kw());
    assert!(Name::from("deprecated").is_attr_kw());
    assert!(Name::from("fallthrough").is_attr_kw());
    assert!(Name::from("indeterminate").is_attr_kw());
    assert!(Name::from("likely").is_attr_kw());
    assert!(Name::from("unlikely").is_attr_kw());
    assert!(Name::from("maybe_unused").is_attr_kw());
    assert!(Name::from("nodiscard").is_attr_kw());
    assert!(Name::from("noreturn").is_attr_kw());
    assert!(Name::from("no_unique_address").is_attr_kw());

    // pp "keywords"
    assert!(!Name::from("elif").is_kw());
    assert!(!Name::from("endif").is_kw());
    assert!(!Name::from("ifdef").is_kw());
    assert!(!Name::from("ifndef").is_kw());
    assert!(!Name::from("elifdef").is_kw());
    assert!(!Name::from("elifndef").is_kw());
    assert!(!Name::from("define").is_kw());
    assert!(!Name::from("undef").is_kw());
    assert!(!Name::from("include").is_kw());
    assert!(!Name::from("embed ").is_kw());
    assert!(!Name::from("line").is_kw());
    assert!(!Name::from("error").is_kw());
    assert!(!Name::from("warning ").is_kw());
    assert!(!Name::from("pragma").is_kw());
    assert!(!Name::from("defined").is_kw());
    assert!(!Name::from("__has_include ").is_kw());
    assert!(!Name::from("__has_cpp_attribute ").is_kw());
    assert!(!Name::from("__has_embed ").is_kw());
    assert!(!Name::from("import").is_kw());
    assert!(!Name::from("module").is_kw());
    assert!(!Name::from("__VA_ARGS__").is_kw());
    assert!(!Name::from("__VA_OPT__").is_kw());

    // alternative tokens
    assert!(!Name::from("and").is_kw());
    assert!(!Name::from("and_eq").is_kw());
    assert!(!Name::from("bitand").is_kw());
    assert!(!Name::from("bitor").is_kw());
    assert!(!Name::from("compl").is_kw());
    assert!(!Name::from("not").is_kw());
    assert!(!Name::from("not_eq").is_kw());
    assert!(!Name::from("or").is_kw());
    assert!(!Name::from("or_eq").is_kw());
    assert!(!Name::from("xor").is_kw());
    assert!(!Name::from("xor_eq").is_kw());

    // other
    assert!(!Name::from("ifelse").is_kw());
    assert!(!Name::from("if2").is_kw());
    assert!(!Name::from("blabla").is_kw());
}
