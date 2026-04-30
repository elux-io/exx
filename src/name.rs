use bumpalo::Bump;
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::{self, Debug, Formatter},
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Name(u32);

impl Debug for Name {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Name({} \"{}\")", self.0, self.as_str())
    }
}

impl Name {
    pub fn from(s: &str) -> Self {
        NAMER.with_borrow_mut(|n| n.add(s))
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: le namer est thread_local et il n'y a qu'un thread donc le
        // str retourné par le namer devrait vivre au moins aussi longtemps que
        // &self
        NAMER.with_borrow(|n| unsafe { &*std::ptr::from_ref(n.get(*self)) })
    }

    pub fn is_kw(self) -> bool {
        self.0 <= kw::While.0
    }

    pub fn is_ctxt_kw(self) -> bool {
        self.0 >= ctxt_kw::Final.0 && self.0 <= ctxt_kw::Pre.0
    }

    pub fn is_attr_kw(self) -> bool {
        self.0 >= attr_kw::Assume.0 && self.0 <= attr_kw::NoUniqueAddress.0
    }
}

macro_rules! predefined_names {
    (
        $(
            $mod:ident {
                $($name:ident: $val:literal),* $(,)?
            }
        )*
    ) => {
        predefined_names!(@mods 0usize; $($mod { $($name: $val),* })*);

        impl Namer {
            fn add_predefined_names(&mut self) {
                $($(self.add($val);)*)*
            }
        }
    };

    // on fait tout ce bazar récursif juste pour avoir l'index "global"
    // (on génère un module à la fois avec $i l'indice de départ pour ce module)
    (@mods $i:expr; $mod:ident { $($name:ident: $val:literal),* } $($tail:tt)*) => {
        // c'est vraiment moche le UPPER_CASE partout et si on utilisait des enums ça
        // serait en PascalCase donc tant pis on les met en PascalCase quand même
        #[allow(non_upper_case_globals)]
        pub mod $mod {
            use crate::name::Name;
            $(pub const $name: Name = Name(($i + ${index()}) as u32);)*
        }

        predefined_names!(@mods $i + [$($val),*].len(); $($tail)*);
    };
    (@mods $i:expr;) => {};
}

predefined_names! {
    kw {
        Alignas: "alignas",
        Alignof: "alignof",
        Asm: "asm",
        Auto: "auto",
        Bool: "bool",
        Break: "break",
        Case: "case",
        Catch: "catch",
        Char: "char",
        Char16T: "char16_t",
        Char32T: "char32_t",
        Char8T: "char8_t",
        Class: "class",
        CoAwait: "co_await",
        Concept: "concept",
        Const: "const",
        ConstCast: "const_cast",
        Consteval: "consteval",
        Constexpr: "constexpr",
        Constinit: "constinit",
        Continue: "continue",
        ContractAssert: "contract_assert",
        CoReturn: "co_return",
        CoYield: "co_yield",
        Decltype: "decltype",
        Default: "default",
        Delete: "delete",
        Do: "do",
        Double: "double",
        DynamicCast: "dynamic_cast",
        Else: "else",
        Enum: "enum",
        Explicit: "explicit",
        Export: "export",
        Extern: "extern",
        False: "false",
        Float: "float",
        For: "for",
        Friend: "friend",
        Goto: "goto",
        If: "if",
        Inline: "inline",
        Int: "int",
        Long: "long",
        Mutable: "mutable",
        Namespace: "namespace",
        New: "new",
        Noexcept: "noexcept",
        Nullptr: "nullptr",
        Operator: "operator",
        Private: "private",
        Protected: "protected",
        Public: "public",
        Register: "register",
        ReinterpretCast: "reinterpret_cast",
        Requires: "requires",
        Return: "return",
        Short: "short",
        Signed: "signed",
        Sizeof: "sizeof",
        Static: "static",
        StaticAssert: "static_assert",
        StaticCast: "static_cast",
        Struct: "struct",
        Switch: "switch",
        Template: "template",
        This: "this",
        ThreadLocal: "thread_local",
        Throw: "throw",
        True: "true",
        Try: "try",
        Typedef: "typedef",
        Typeid: "typeid",
        Typename: "typename",
        Union: "union",
        Unsigned: "unsigned",
        Using: "using",
        Virtual: "virtual",
        Void: "void",
        Volatile: "volatile",
        WcharT: "wchar_t",
        While: "while",
    }

    ctxt_kw {
        Final: "final",
        Import: "import",
        Module: "module",
        Override: "override",
        Post: "post",
        Pre: "pre",
    }

    attr_kw {
        Assume: "assume",
        Deprecated: "deprecated",
        Fallthrough: "fallthrough",
        Indeterminate: "indeterminate",
        Likely: "likely",
        Unlikely: "unlikely",
        MaybeUnused: "maybe_unused",
        Nodiscard: "nodiscard",
        Noreturn: "noreturn",
        NoUniqueAddress: "no_unique_address",
    }

    pp_kw {
        Elif: "elif",
        Endif: "endif",
        Ifdef: "ifdef",
        Ifndef: "ifndef",
        Elifdef: "elifdef",
        Elifndef: "elifndef",
        Define: "define",
        Defined: "defined",
        Undef: "undef",
        Include: "include",
        Embed: "embed",
        Line: "line",
        Error: "error",
        Warning: "warning",
        Pragma: "pragma",
        _Pragma: "_Pragma",
        HasInclude: "__has_include",
        HasCppAttribute: "__has_cpp_attribute",
        HasEmbed: "__has_embed",
        VaArgs: "__VA_ARGS__",
        VaOpt: "__VA_OPT__",
    }
}

thread_local! {
    static NAMER: RefCell<Namer> = {
        let mut namer = Namer::new();
        namer.add_predefined_names();
        RefCell::new(namer)
    };
}

struct Namer {
    bump: Bump,
    names: HashMap<&'static str, Name>,
    strs: Vec<&'static str>,
}

impl Namer {
    fn new() -> Self {
        // ces capacités sont choisies au pif
        // todo: les choisir mieux
        Self {
            bump: Bump::with_capacity(32_768),
            names: HashMap::with_capacity(1024),
            strs: Vec::with_capacity(1024),
        }
    }

    fn add(&mut self, s: &str) -> Name {
        if let Some(&name) = self.names.get(s) {
            return name;
        }

        let name = Name(self.strs.len() as u32);
        let s = self.bump.alloc_str(s);
        // SAFETY: on ne peut pas accéder à ce &str en dehors du namer donc
        // même si on met une lifetime static on sait que ça vivra assez longtemps
        let s: &'static str = unsafe { &*std::ptr::from_ref(s) };

        self.names.insert(s, name);
        self.strs.push(s);
        name
    }

    fn get(&self, name: Name) -> &str {
        self.strs[name.0 as usize]
    }
}
