#![feature(custom_inner_attributes)]
#![rustfmt::skip]

use std::{collections::HashMap, ops::Range, path::{Path, PathBuf}, sync::{LazyLock, RwLock}};
use chrono::Utc;
use exx::{diag::Diags, lex::{CharError, EscapeError, HeaderKind, LexError, ParseNumberError, TokenKind, Token}, name::{Name, kw, pp_kw}, pp::*, source::{FileLoader, LoadError, Loc, SourceHub, Span}};

struct Run {
    tokens: Vec<Token>,
    shub: SourceHub,
    diags: Diags,
}

static FILES: LazyLock<RwLock<HashMap<PathBuf, Result<Vec<u8>, LoadError>>>> = LazyLock::new(|| {
    let mut files = HashMap::new();
    let mut file = |path, c| {
        let path = root_path().join(Path::new(path).components().collect::<PathBuf>());
        files.insert(path, c);
    };

    // includes accessibles avec <>
    file("angle_includes/vector", Ok("class vector {};".into()));
    file("angle_includes/angle_include.hpp", Ok("class foo {};".into()));
    file("angle_includes/unreadable.hpp", Err(LoadError::Unreadable));
    // includes accessibles uniquement avec ""
    file("quote_includes/quote_include.hpp", Ok("HELLO".into()));

    // embeds accessibles avec <>
    file("angle_embeds/angle_embed.dat", Ok(vec![3]));
    file("angle_embeds/unreadable.dat", Err(LoadError::Unreadable));
    file("angle_embeds/empty.dat", Ok(vec![]));
    // embeds accessibles uniquement avec ""
    file("quote_embeds/quote_embed.dat", Ok(vec![4, 2, 1]));

    // dossier local (celui de main.cpp)
    file("src/local.dat", Ok(vec![8, 3, 7, 2, 4]));
    file("src/macro.hpp", Ok("#define MACRO 42".into()));
    file("src/non_utf8.hpp", Ok(vec![128]));

    // on ajoute un main.cpp bidon juste pour qu'il existe mais on s'embête pas à
    // écrire le contenu de chaque test dedans car on s'en fout (on a directement
    // la string qui contient le texte)
    file("src/main.cpp", Ok("".into()));

    RwLock::new(files)
});

fn root_path() -> &'static Path {
    Path::new("root")
}

/// pour ajouter des fichiers spécifiques à chaque test si besoin
fn add_file(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    FILES.write().unwrap().insert(root_path().join(path), Ok(contents.as_ref().into()));
}

// on n'utilise pas le vrai filesystem pour les tests (peut-être qu'on devrait ?)
struct TestFileLoader;
impl FileLoader for TestFileLoader {
    fn load(&self, path: &Path) -> Result<Vec<u8>, LoadError> {
        FILES.read().unwrap().get(path).unwrap_or(&Err(LoadError::NotFound)).clone()
    }
}

fn preprocess(src: &str, opts: PpOptions) -> Run {
    let mut run = Run {
        tokens: Vec::new(),
        shub: SourceHub::new(),
        diags: Diags::new(),
    };
    let root = root_path();
    let source_id = run.shub.add_source(root.join("src").join("main.cpp"), src.to_owned()).id();

    let mut pp = Preprocessor::new(opts, &mut run.shub, &mut run.diags, &TestFileLoader);
    pp.add_include_dir(root.join("angle_includes"), HeaderKind::Angle);
    pp.add_include_dir(root.join("quote_includes"), HeaderKind::Quote);
    pp.add_embed_dir(root.join("angle_embeds"), HeaderKind::Angle);
    pp.add_embed_dir(root.join("quote_embeds"), HeaderKind::Quote);

    // on veut changer la loc car lors de la construction du pp il va ajouter
    // les macros prédéfinies etc et ça va consommer des Loc, ce qui fait qu'on
    // ne sait pas quelle sera la prochaine Loc et en plus elle changera si on
    // rajoute des macros prédéfinies, ce qui est chiant vu que ça va casser les
    // tests, il faudrait mettre à jour tous les spans qui proviennent d'une
    // expansion de macro etc, pour qu'ils se basent sur la nouvelle Loc
    // du coup pour éviter ça on force la prochaine Loc à une valeur lointaine que
    // le pp n'atteindra pas même en rajoutant plein de macros ou autre
    pp.shub.set_next_loc(Loc(10_000_000));

    run.tokens = pp.preprocess(source_id);
    run
}

fn full_path(path: &str) -> String {
    let path = root_path().join(Path::new(path).components().collect::<PathBuf>());
    // c'est pas comme ça qu'il faut escape mais osef c'est juste pour les tests
    String::from_utf8_lossy(&path.as_os_str().as_encoded_bytes().escape_ascii().collect::<Vec<_>>()).into_owned()
}

fn span(range: Range<u32>) -> Span {
    Span {
        lo: Loc(range.start),
        hi: Loc(range.end),
    }
}

/// assert que la sortie du préprocesseur est celle attendue et qu'il n'y a pas d'erreurs
macro_rules! pp {
    ($src:expr, $expected:expr) => {
        pp!(PpOptions::default(), $src, $expected);
    };

    ($opts:expr, $src:expr, $expected:expr) => {{
        let run = preprocess($src, $opts);
        assert_eq!(run.diags.diags(), &[]);
        let actual = format_pp_output(&run.tokens, &run.shub);
        assert_eq!(actual.trim(), $expected.trim());
    }};
}

/// assert que les erreurs sont celles attendues
macro_rules! diags {
    ($src:expr, [$($diags:expr),* $(,)?]) => {
        diags!(PpOptions::default(), $src, [$($diags),*]);
    };

    ($opts:expr, $src:expr, [$($diags:expr),* $(,)?]) => {
        let run = preprocess($src, $opts);
        let actual = run.diags.diags();
        let expected = [$($diags.into()),*];
        assert_eq!(actual, &expected);
    };
}

/// assert que la sortie et les erreurs sont celles attendues
macro_rules! pp_and_diags {
    ($src:expr, $expected_output:expr, [$($diags:expr),* $(,)?]) => {{
        let run = preprocess($src, PpOptions::default());
        let actual_output = format_pp_output(&run.tokens, &run.shub);
        assert_eq!(actual_output.trim(), $expected_output.trim());

        let actual_diags = run.diags.diags();
        let expected_diags = [$($diags.into()),*];
        assert_eq!(actual_diags, &expected_diags);
    }};
}

#[test]
fn predefined_macros() {
    pp!("__cplusplus", "202600L");
    pp!("__FILE__", format!("\"{}\"", full_path("src/main.cpp")));
    pp!("__LINE__", "1");
    pp!("__STDC_EMBED_NOT_FOUND__", "0");
    pp!("__STDC_EMBED_FOUND__", "1");
    pp!("__STDC_EMBED_EMPTY__", "2");
    pp!("__STDC_HOSTED__", "0");
    pp!("__STDCPP_DEFAULT_NEW_ALIGNMENT__", "16uz");

    let now = Utc::now();
    pp!("__DATE__", now.format("\"%b %e %Y\"").to_string());
    pp!("__TIME__", now.format("\"%H:%M:%S\"").to_string());
}

#[test]
fn common_predefined_macros() {
    let opts = || PpOptions {
        common_defines: true,
        ..Default::default()
    };

    pp!(opts(), "__EXX__", "1");

    // __BASE_FILE__
    add_file("src/base_file.hpp", "
        __BASE_FILE__
    ");
    let main_path = full_path("src/main.cpp");
    let src = r#"
        __BASE_FILE__
        #include "base_file.hpp"
    "#;
    pp!(opts(), src, format!(r#"
        "{main_path}"
        "{main_path}"
    "#));

    // pas affecté par une directive #line (GCC fait pareil, Clang affiche "bla")
    let src = r#"
        #line 1 "bla"
        __BASE_FILE__
    "#;
    pp!(opts(), src, format!(r#"
        "{main_path}"
    "#));

    // __FILE_NAME__
    pp!(opts(), "__FILE_NAME__", "\"main.cpp\"");

    // affecté par une directive #line
    let src = r#"
        #line 1 "bla"
        __FILE_NAME__
    "#;
    pp!(opts(), src, "\"bla\"");

    // __COUNTER__
    add_file("src/counter.hpp", "
        __COUNTER__
    ");
    let src = r#"
        __COUNTER__
        #include "counter.hpp"
        __COUNTER__
    "#;
    pp!(opts(), src, "
        0
        1
        2
    ");

    // __TIMESTAMP__
    pp!(opts(), "__TIMESTAMP__", "\"??? ??? ?? ??:??:?? ????\"");

    // __INCLUDE_LEVEL__
    add_file("src/include_level.hpp", "
        __INCLUDE_LEVEL__
    ");
    let src = r#"
        __INCLUDE_LEVEL__
        #include "include_level.hpp"
        __INCLUDE_LEVEL__
    "#;
    pp!(opts(), src, "
        0
        1
        0
    ");

    // pas d'effet si désactivé
    let src = "
        __EXX__
        __BASE_FILE__
        __FILE_NAME__
        __COUNTER__
        __TIMESTAMP__
        __INCLUDE_LEVEL__
    ";
    pp!(src, "
        __EXX__
        __BASE_FILE__
        __FILE_NAME__
        __COUNTER__
        __TIMESTAMP__
        __INCLUDE_LEVEL__
    ");
}

#[test]
fn r#if() {
    let src = "
        #if true
        A
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
        A
        #endif
        B
    ";
    pp!(src, "B");

    let src = "
        #if true
        A
        #else
        B
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
        A
        #else
        B
        #endif
    ";
    pp!(src, "B");

    // le #else n'est pas au début de la ligne donc ce n'est pas un #else
    let src = "
        #if true
        A #else
        B
        #else
        C
        #endif
    ";
    pp!(src, "
        A #else
        B
    ");
    let src = "
        #if false
        A #else
        B
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // imbriqué dans un #if
    let src = "
        #if true
            #if true
            A
            #else
            B
            #endif
        #else
        C
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
            #if true
            A
            #else
            B
            #endif
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // pas d'erreur lexicale dans un bloc faux
    let src = r"
        #if false
        ''
        \N{blabla}
        \u0065
        #endif
    ";
    pp!(src, "");

    // ni toute autre erreur
    let src = "
        #if false
        #error blabla
        #endif
    ";
    pp!(src, "");

    // on se fait pas avoir par un #else dans un commentaire
    let src = "
        #if false
        A
        /*
        #else
        */
        B
        #endif
    ";
    pp!(src, "");

    let src = "
        #if false
        A
        #
        #else
        B
        #endif
    ";
    pp!(src, "B");

    let src = "
        #if false
        A
        ##else
        ''
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // le premier else n'est pas un vrai #else (null directive)
    let src = "
        #if false
        #
        else
        #error pas une erreur car toujours dans le if false
        #else
        A
        #endif
    ";
    pp!(src, "A");

    // erreur si pas d'expression (sur la même ligne)
    let src = "
        #if
        true
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: kw::If, span: span(10..12) }]);

    // erreur aussi si pas d'expression après expansion
    let src = "
        #define EMPTY
        #if EMPTY
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: kw::If, span: span(32..34) }]);

    // pas de #endif
    diags!("#if true", [NoEndif { directive: kw::If, span: span(1..3) }]);
    diags!("#if false", [NoEndif { directive: kw::If, span: span(1..3) }]);
    // plusieurs
    let src = "
        #if true
        #if true
    ";
    diags!(src, [
        NoEndif { directive: kw::If, span: span(10..12) },
        NoEndif { directive: kw::If, span: span(27..29) },
    ]);
    // ici il n'y a pas d'erreur sur le 2ème #if car il est dans un bloc faux et
    // on ignore ce qu'il y a dans un bloc faux
    // todo: peut-être qu'on devrait afficher l'erreur quand même ?
    // Clang/GCC/EDG l'affichent
    let src = "
        #if false
        #if true
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
}

#[test]
fn ifdef() {
    let src = "
        #define FOO
        #ifdef FOO
        A
        #endif
    ";
    pp!(src, "A");

    let src = "
        #ifdef FOO
        A
        #endif
        B
    ";
    pp!(src, "B");

    let src = "
        #define FOO
        #ifdef FOO
        A
        #else
        B
        #endif
    ";
    pp!(src, "A");

    let src = "
        #ifdef FOO
        A
        #else
        B
        #endif
    ";
    pp!(src, "B");

    // erreur si pas d'expression (sur la même ligne)
    let src = "
        #ifdef
        FOO
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Ifdef, span: span(10..15) }]);

    // macro invalide
    let src = "
        #ifdef +
        #endif
    ";
    diags!(src, [InvalidMacName { lexeme: "+".to_owned(), span: span(16..17), is_name: false }]);

    // tokens after directive
    let src = "
        #ifdef FOO blabla
        #endif
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(20..26)] }]);

    // la macro peut avoir un body vide
    let src = "
        #define EMPTY
        #ifdef EMPTY
        A
        #endif
    ";
    pp!(src, "A");

    // pas de #endif
    diags!("#ifdef FOO", [NoEndif { directive: pp_kw::Ifdef, span: span(1..6) }]);
}

#[test]
fn ifndef() {
    let src = "
        #define FOO
        #ifndef FOO
        A
        #endif
        B
    ";
    pp!(src, "B");

    let src = "
        #ifndef FOO
        A
        #endif
    ";
    pp!(src, "A");

    let src = "
        #define FOO
        #ifndef FOO
        A
        #else
        B
        #endif
    ";
    pp!(src, "B");

    let src = "
        #ifndef FOO
        A
        #else
        B
        #endif
    ";
    pp!(src, "A");

    // erreur si pas d'expression (sur la même ligne)
    let src = "
        #ifndef
        FOO
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Ifndef, span: span(10..16) }]);

    // macro invalide
    let src = "
        #ifndef +
        #endif
    ";
    diags!(src, [InvalidMacName { lexeme: "+".to_owned(), span: span(17..18), is_name: false }]);

    // tokens after directive
    let src = "
        #ifndef FOO blabla
        #endif
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(21..27)] }]);

    // la macro peut avoir un body vide
    let src = "
        #define EMPTY
        #ifndef EMPTY
        A
        #else
        B
        #endif
    ";
    pp!(src, "B");

    // pas de #endif
    diags!("#ifndef FOO", [NoEndif { directive: pp_kw::Ifndef, span: span(1..7) }]);
}

#[test]
fn elif() {
    let src = "
        #if true
        A
        #elif true
        B
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
        A
        #elif true
        B
        #endif
    ";
    pp!(src, "B");

    let src = "
        #if false
        A
        #elif false
        B
        #endif
        C
    ";
    pp!(src, "C");

    let src = "
        #if true
        A
        #elif true
        B
        #else
        C
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
        A
        #elif true
        B
        #else
        C
        #endif
    ";
    pp!(src, "B");

    let src = "
        #if false
        A
        #elif false
        B
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // imbriqué dans un #if
    let src = "
        #if true
            #if true
            A
            #elif true
            B
            #else
            C
            #endif
        #elif true
        D
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
            #if true
            A
            #elif true
            B
            #else
            C
            #endif
        #elif true
        D
        #endif
    ";
    pp!(src, "D");

    // erreur si pas d'expression (sur la même ligne)
    let src = "
        #if false
        #elif
        true
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Elif, span: span(28..32) }]);

    // erreur aussi si pas d'expression après expansion
    let src = "
        #define EMPTY
        #if false
        #elif EMPTY
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Elif, span: span(50..54) }]);

    // pas dans un if
    diags!("#elif true", [NoIf { directive: pp_kw::Elif, span: span(1..5) }]);
    diags!("#elif", [NoIf { directive: pp_kw::Elif, span: span(1..5) }]);

    // la condition n'est pas évaluée si on était rentré dans le bloc précédent
    // (donc pas d'erreur)
    let src = "
        #if true
        A
        #elif ceci est une expression invalide
        B
        #elif
        C
        #endif
    ";
    pp!(src, "A");

    // pas de #endif (c'est le #if qui est considéré non terminé)
    let src = "
        #if true
        #elif true
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
    let src = "
        #if false
        #elif true
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
}

#[test]
fn elifdef() {
    let src = "
        #define FOO
        #if true
        A
        #elifdef FOO
        B
        #endif
    ";
    pp!(src, "A");

    let src = "
        #define FOO
        #if false
        A
        #elifdef FOO
        B
        #endif
    ";
    pp!(src, "B");

    let src = "
        #if false
        A
        #elifdef FOO
        B
        #endif
        C
    ";
    pp!(src, "C");

    let src = "
        #define FOO
        #if true
        A
        #elifdef FOO
        B
        #else
        C
        #endif
    ";
    pp!(src, "A");

    let src = "
        #define FOO
        #if false
        A
        #elifdef FOO
        B
        #else
        C
        #endif
    ";
    pp!(src, "B");

    let src = "
        #if false
        A
        #elifdef FOO
        B
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // imbriqué dans un #if
    let src = "
        #define FOO
        #if true
            #if true
            A
            #elifdef FOO
            B
            #else
            C
            #endif
        #elifdef FOO
        D
        #endif
    ";
    pp!(src, "A");

    let src = "
        #define FOO
        #if false
            #if true
            A
            #elifdef FOO
            B
            #else
            C
            #endif
        #elifdef FOO
        D
        #endif
    ";
    pp!(src, "D");

    // erreur si pas d'expression (sur la même ligne)
    let src = "
        #if false
        #elifdef
        FOO
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Elifdef, span: span(28..35) }]);

    // macro invalide
    let src = "
        #if false
        #elifdef +
        #endif
    ";
    diags!(src, [InvalidMacName { lexeme: "+".to_owned(), span: span(36..37), is_name: false }]);

    // tokens after directive
    let src = "
        #if false
        #elifdef FOO blabla
        #endif
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(40..46)] }]);

    // la macro peut avoir un body vide
    let src = "
        #define EMPTY
        #if false
        A
        #elifdef EMPTY
        B
        #endif
    ";
    pp!(src, "B");

    // pas dans un if
    diags!("#elifdef FOO", [NoIf { directive: pp_kw::Elifdef, span: span(1..8) }]);
    diags!("#elifdef", [NoIf { directive: pp_kw::Elifdef, span: span(1..8) }]);

    // la condition n'est pas évaluée si on était rentré dans le bloc précédent
    let src = "
        #if true
        A
        #elifdef bla + bla invalide
        B
        #elifdef
        C
        #endif
    ";
    pp!(src, "A");

    // pas de #endif (c'est le #if qui est considéré non terminé)
    let src = "
        #if true
        #elifdef FOO
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
    let src = "
        #if false
        #elifdef FOO
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
}

#[test]
fn elifndef() {
    let src = "
        #if true
        A
        #elifndef FOO
        B
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
        A
        #elifndef FOO
        B
        #endif
    ";
    pp!(src, "B");

    let src = "
        #define FOO
        #if false
        A
        #elifndef FOO
        B
        #endif
        C
    ";
    pp!(src, "C");

    let src = "
        #if true
        A
        #elifndef FOO
        B
        #else
        C
        #endif
    ";
    pp!(src, "A");

    let src = "
        #if false
        A
        #elifndef FOO
        B
        #else
        C
        #endif
    ";
    pp!(src, "B");

    let src = "
        #define FOO
        #if false
        A
        #elifndef FOO
        B
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // imbriqué dans un #if
    let src = "
        #if true
            #if true
            A
            #elifndef FOO
            B
            #else
            C
            #endif
        #elifndef FOO
        D
        #endif
    ";
    pp!(src, "A");
    let src = "
        #if false
            #if true
            A
            #elifndef FOO
            B
            #else
            C
            #endif
        #elifndef FOO
        D
        #endif
    ";
    pp!(src, "D");

    // erreur si pas d'expression (sur la même ligne)
    let src = "
        #if false
        #elifndef
        FOO
        #endif
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Elifndef, span: span(28..36) }]);

    // macro invalide
    let src = "
        #if false
        #elifndef +
        #endif
    ";
    diags!(src, [InvalidMacName { lexeme: "+".to_owned(), span: span(37..38), is_name: false }]);

    // tokens after directive
    let src = "
        #if false
        #elifndef FOO blabla
        #endif
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(41..47)] }]);

    // la macro peut avoir un body vide
    let src = "
        #define EMPTY
        #if false
        A
        #elifndef EMPTY
        B
        #else
        C
        #endif
    ";
    pp!(src, "C");

    // pas dans un if
    diags!("#elifndef FOO", [NoIf { directive: pp_kw::Elifndef, span: span(1..9) }]);
    diags!("#elifndef", [NoIf { directive: pp_kw::Elifndef, span: span(1..9) }]);

    // la condition n'est pas évaluée si on était rentré dans le bloc précédent
    let src = "
        #if true
        A
        #elifndef bla + bla invalide
        B
        #elifndef
        C
        #endif
    ";
    pp!(src, "A");

    // pas de #endif (c'est le #if qui est considéré non terminé)
    let src = "
        #if true
        #elifndef FOO
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
    let src = "
        #if false
        #elifndef FOO
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
}

#[test]
fn r#else() {
    // pas dans un if
    diags!("#else", [NoIf { directive: kw::Else, span: span(1..5) }]);

    // c'est une erreur qu'il y ait des tokens en trop même si le bloc précédent
    // était à true
    let src = "
        #if true
        #else 3
        #endif
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(32..33)] }]);

    // mais tout est ignoré dans un bloc faux
    let src = "
        #if false
            #if true
            #else 3
            #endif
        #endif
    ";
    pp!(src, "");

    // else après un else
    let src = "
        #if true
        #else
        #else
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: kw::Else, span: span(41..45) }]);
    // elif
    let src = "
        #if true
        #else
        #elif true
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elif, span: span(41..45) }]);
    // elifdef
    let src = "
        #if true
        #else
        #elifdef FOO
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elifdef, span: span(41..48) }]);
    // elifndef
    let src = "
        #if true
        #else
        #elifndef FOO
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elifndef, span: span(41..49) }]);

    // idem si la condition est fausse
    let src = "
        #if false
        #else
        #else
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: kw::Else, span: span(42..46) }]);
    let src = "
        #if false
        #else
        #elif true
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elif, span: span(42..46) }]);
    let src = "
        #if false
        #else
        #elifdef FOO
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elifdef, span: span(42..49) }]);
    let src = "
        #if false
        #else
        #elifndef FOO
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elifndef, span: span(42..50) }]);

    // on n'affiche pas l'erreur sur le fait que la directive est invalide,
    // juste celle sur le else/elif/etc en trop car on s'en fout qu'elle soit
    // valide ou pas, elle ne devrait pas y être tout court
    let src = "
        #if false
        #else
        #else blabla
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: kw::Else, span: span(42..46) }]);
    let src = "
        #if false
        #else
        #elif
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elif, span: span(42..46) }]);
    let src = "
        #if false
        #else
        #elifdef
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elifdef, span: span(42..49) }]);
    let src = "
        #if false
        #else
        #elifndef
        #endif
    ";
    diags!(src, [InvalidDirectiveAfterElse { directive: pp_kw::Elifndef, span: span(42..50) }]);

    // pas de #endif (c'est le #if qui est considéré non terminé)
    let src = "
        #if true
        #else
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
    let src = "
        #if false
        #else
    ";
    diags!(src, [NoEndif { directive: kw::If, span: span(10..12) }]);
}

#[test]
fn endif() {
    // pas dans un if
    diags!("#endif", [NoIf { directive: pp_kw::Endif, span: span(1..6) }]);

    // extra tokens
    let src = "
        #if true
        #endif bla
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(33..36)] }]);
    let src = "
        #if false
        #endif bla
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(34..37)] }]);
}

#[test]
fn include() {
    // fichier dans l'include dir des headers angle ("système")
    pp!("#include <angle_include.hpp>", "class foo {};");
    // on peut aussi y accéder avec un header quote
    pp!("#include \"angle_include.hpp\"", "class foo {};");

    // fichier dans le dossier local
    let src = r#"
        #include "macro.hpp"
        MACRO
    "#;
    pp!(src, "42");
    // introuvable avec un header angle
    diags!("#include <macro.hpp>", [InvalidHeader { error: HeaderError::NotFound("macro.hpp".into()), span: span(9..20) }]);

    // pas dans le dossier local mais ajouté comme include dir quote
    pp!("#include \"quote_include.hpp\"", "HELLO");
    diags!("#include <quote_include.hpp>", [InvalidHeader { error: HeaderError::NotFound("quote_include.hpp".into()), span: span(9..28) }]);

    // fichier ajouté dans un embed dir quote donc pas accessible aux includes
    diags!("#include \"quote_embed.dat\"", [InvalidHeader { error: HeaderError::NotFound("quote_embed.dat".into()), span: span(9..26) }]);
    diags!("#include <quote_embed.dat>", [InvalidHeader { error: HeaderError::NotFound("quote_embed.dat".into()), span: span(9..26) }]);
    // fichier ajouté dans un embed dir angle donc pas accessible aux includes
    diags!("#include \"angle_embed.dat\"", [InvalidHeader { error: HeaderError::NotFound("angle_embed.dat".into()), span: span(9..26) }]);
    diags!("#include <angle_embed.dat>", [InvalidHeader { error: HeaderError::NotFound("angle_embed.dat".into()), span: span(9..26) }]);

    // fichier unreadable
    diags!("#include <unreadable.hpp>", [InvalidHeader { error: HeaderError::Unreadable("unreadable.hpp".into()), span: span(9..25) }]);

    // le fichier `shadowed.hpp` est accessible avec des headers angle mais il
    // est aussi présent dans le dossier local
    add_file("angle_includes/shadowed.hpp", "class original {};");
    add_file("src/shadowed.hpp", "class custom {};");
    pp!("#include \"shadowed.hpp\"", "class custom {};");
    pp!("#include <shadowed.hpp>", "class original {};");

    // le fichier doit être de l'utf8 valide
    diags!("#include \"non_utf8.hpp\"", [InvalidHeader { error: HeaderError::Unreadable("non_utf8.hpp".into()), span: span(9..23) }]);

    // quand le header name est écrit directement, tout ce qui est entre <> ou ""
    // est conservé à l'identique (espaces, backslashes, commentaires, ...)
    // (c'est implementation-defined mais c'est comme ça que ça se comporte ici)
    let src = r#"
        #include <  bla"\257 \u0065 ' \  // /* lol */   >
    "#;
    diags!(src, [InvalidHeader { error: HeaderError::NotFound(r#"  bla"\257 \u0065 ' \  // /* lol */   "#.into()), span: span(18..58) }]);
    let src = r#"
        #include "  bla\257 \u0065 ' \  // /* lol */   "
    "#;
    diags!(src, [InvalidHeader { error: HeaderError::NotFound(r#"  bla\257 \u0065 ' \  // /* lol */   "#.into()), span: span(18..57) }]);

    // pour les headers angle créés à partir d'une expansion, les commentaires
    // sont ignorés et les espaces sont remplacés par 1 espace
    // (pareil c'est implementation-defined)
    let src = "
        #define H <   bla   /* abc */  >
        #include H
    ";
    diags!(src, [InvalidHeader { error: HeaderError::NotFound(" bla ".into()), span: span(10000000..10000022) }]);
    let src = "
        #define A <
        #include A bla   /* abc */  >
    ";
    diags!(src, [InvalidHeader { error: HeaderError::NotFound(" bla ".into()), span: span(38..58) }]);
    let src = "
        #define A >
        #include <  bla   /* abc */  A
    ";
    diags!(src, [InvalidHeader { error: HeaderError::NotFound(" bla ".into()), span: span(38..59) }]);
    // sans espaces
    let src = "
        #define H <bla>
        #include H
    ";
    diags!(src, [InvalidHeader { error: HeaderError::NotFound("bla".into()), span: span(10000000..10000005) }]);

    // pour les headers quote créés à partir d'une expansion, ça se comporte comme
    // une string (c'en est une)
    let src = r#"
        #define H "   bla /* abc */ "
        #include H
    "#;
    diags!(src, [InvalidHeader { error: HeaderError::NotFound("   bla /* abc */ ".into()), span: span(10000000..10000019) }]);
    // donc erreur si string invalide
    // todo: peut-être qu'on veut pas afficher en plus l'erreur invalid header ?
    let src = r#"
        #define H "\+"
        #include H
    "#;
    diags!(src, [
        InvalidHeader { error: HeaderError::NotFound(r"\+".into()), span: span(10000000..10000004) },
        LexError::Escape(EscapeError::UnknownEscape, span(20..21)),
    ]);

    // on utilise le lexème de la string (avec les line conts enlevés et UCN décodés),
    // pas la valeur en elle-même
    // todo: peut-être qu'il faut pas faire ça ? les autres compilateurs ne décodent
    // pas les UCN
    let src = r#"
        #define H "bl\u0061\\bla\t"
        #include H
    "#;
    diags!(src, [InvalidHeader { error: HeaderError::NotFound(r"bla\\bla\t".into()), span: span(10000000..10000017) }]);

    // avec line continuations
    let src = r"
        #include <bla \
            bla>
    ";
    diags!(src, [InvalidHeader { error: HeaderError::NotFound("bla             bla".into()), span: span(18..41) }]);

    // header vide
    let src = "
        #define H <>
        #include H
        #include <>
    ";
    diags!(src, [
        InvalidHeader { error: HeaderError::Empty, span: span(10000000..10000002) },
        InvalidHeader { error: HeaderError::Empty, span: span(58..60) },
    ]);
    let src = r#"
        #define H ""
        #include H
        #include ""
    "#;
    diags!(src, [
        InvalidHeader { error: HeaderError::Empty, span: span(10000000..10000002) },
        InvalidHeader { error: HeaderError::Empty, span: span(58..60) },
    ]);

    // limite d'include depth
    add_file("src/recursive.hpp", "#include \"recursive2.hpp\"");
    add_file("src/recursive2.hpp", "#include \"recursive.hpp\"");
    let opts = PpOptions {
        max_include_depth: 8,
        ..Default::default()
    };
    diags!(opts, "#include \"recursive.hpp\"", [ExceededMaxIncludeDepth { max: 8, span: span(10000435..10000442) }]);

    // incomplet
    diags!("#include", [ExpectedTokensInDirective { directive: pp_kw::Include, span: span(1..8) }]);
    let src = "
        #define EMPTY
        #include EMPTY
    ";
    diags!(src, [ExpectedTokensInDirective { directive: pp_kw::Include, span: span(32..39) }]);

    // header malformé
    diags!("#include +", [InvalidHeader { error: HeaderError::Malformed, span: span(9..10) }]);

    // extra tokens
    diags!("#include <vector> bla", [TokensAfterDirective { spans: vec![span(18..21)] }]);
    diags!("#include \"macro.hpp\" bla", [TokensAfterDirective { spans: vec![span(21..24)] }]);
    let src = "
        #define V <vector>
        #include V bla bla
    ";
    diags!(src, [TokensAfterDirective { spans: vec![span(47..50), span(51..54)] }]);
    let src = r#"
        #define V "vector" bla bla
        #include V
    "#;
    diags!(src, [TokensAfterDirective { spans: vec![span(10000009..10000012), span(10000013..10000016)] }]);

    // extra tokens + invalid header (le span du invalid header pointe juste sur
    // le header en lui-même, pas tous les tokens)
    let src = "
        #include <bla> 1 + 2
    ";
    diags!(src, [
        TokensAfterDirective { spans: vec![span(24..25), span(26..27), span(28..29)] },
        InvalidHeader { error: HeaderError::NotFound("bla".into()), span: span(18..23) },
    ]);

    // les string literals ne sont pas encore concaténés
    let src = r#"
        #include "vec" "tor"
    "#;
    diags!(src, [
        TokensAfterDirective { spans: vec![span(24..29)] },
        InvalidHeader { error: HeaderError::NotFound("vec".into()), span: span(18..23) },
    ]);

    // chemins plus complexes
    add_file("angle_includes/sub_folder/inner.hpp", r#"
        #include "../vector"
    "#);
    pp!("#include <sub_folder/./inner.hpp>", "class vector {};");
}

#[test]
fn has_include() {
    // ne peut apparaitre que dans des directives conditionnelles
    diags!("__has_include", [ForbiddenHasExpr { name: pp_kw::HasInclude, span: span(0..13) }]);
    // d'après [cpp.cond] (paragraphe 7) il ne devrait même pas apparaître dans
    // un #define (même si les autres compilateurs acceptent)
    diags!("#define A __has_include", [ForbiddenHasExpr { name: pp_kw::HasInclude, span: span(10..23) }]);
    // interdit quelque soit la façon dont c'est généré
    let src = "
        #define CC(a, b) a ## b
        CC(__has_inc, lude)
    ";
    diags!(src, [ForbiddenHasExpr { name: pp_kw::HasInclude, span: span(10000021..10000034) }]);

    // on ne peut pas le define ou undef
    diags!("#define __has_include", [RedefinedPredefMac { name: pp_kw::HasInclude, span: span(8..21), is_define: true }]);
    diags!("#undef __has_include", [RedefinedPredefMac { name: pp_kw::HasInclude, span: span(7..20), is_define: false }]);

    // parenthèses manquantes
    let src = "
        #if __has_include
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::HasInclude, span: span(13..26), has_parens: false }]);
    let src = "
        #if __has_include(
        #endif
    ";
    diags!(src, [UnmatchedParenL { span: span(26..27) }]);
    let src = "
        #if __has_include()
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::HasInclude, span: span(26..28), has_parens: true }]);

    // considéré comme une macro définie
    let src = "
        #ifdef __has_include
        YES
        #endif
    ";
    pp!(src, "YES");

    // include trouvé dans un include dir angle
    let src = "
        #if __has_include(<angle_include.hpp>)
        YES
        #endif
    ";
    pp!(src, "YES");
    // trouvable avec des quotes aussi
    let src = r#"
        #if __has_include("angle_include.hpp")
        YES
        #endif
    "#;
    pp!(src, "YES");

    // fichier dans le dossier local
    let src = r#"
        #if __has_include("macro.hpp")
        YES
        #endif
    "#;
    pp!(src, "YES");
    // introuvable avec un header angle
    let src = "
        #if __has_include(<macro.hpp>)
        YES
        #endif
    ";
    pp!(src, "");

    // pas dans le dossier local mais ajouté comme include dir quote
    let src = r#"
        #if __has_include("quote_include.hpp")
        YES
        #endif
    "#;
    pp!(src, "YES");
    // introuvable avec header angle
    let src = "
        #if __has_include(<quote_include.hpp>)
        YES
        #endif
    ";
    pp!(src, "");

    // on considère que le fichier non-utf8 non seulement ne peut pas être inclut,
    // mais c'est même ill-formed (d'après [cpp.include] paragraphe 6)
    let src = r#"
        #if __has_include("non_utf8.hpp")
        #endif
    "#;
    diags!(src, [InvalidHeader { error: HeaderError::Unreadable("non_utf8.hpp".into()), span: span(27..41) }]);

    // ill-formed aussi si le fichier n'est pas lisible
    let src = "
        #if __has_include(<unreadable.hpp>)
        #endif
    ";
    diags!(src, [InvalidHeader { error: HeaderError::Unreadable("unreadable.hpp".into()), span: span(27..43) }]);

    // fichier inexistant
    let src = r#"
        #if __has_include(<nope>) || __has_include("nope")
        YES
        #endif
    "#;
    pp!(src, "");

    // fichier ajouté dans un embed dir quote donc pas accessible aux includes
    let src = r#"
        #if __has_include("quote_embed.hpp") || __has_include(<quote_embed.hpp>)
        YES
        #endif
    "#;
    pp!(src, "");
    // fichier ajouté dans un embed dir angle donc pas accessible aux includes
    let src = r#"
        #if __has_include("angle_embed.hpp") || __has_include(<angle_embed.hpp>)
        YES
        #endif
    "#;
    pp!(src, "");

    // on peut inclure __FILE__
    let src = "
        #if __has_include(__FILE__)
        YES
        #endif
    ";
    pp!(src, "YES");
    // mais pas si il correspond à un truc qui existe pas
    let src = r#"
        #line 1 "nope"
        #if __has_include(__FILE__)
        YES
        #endif
    "#;
    pp!(src, "");

    // pas expandé car c'est un header-name
    let src = "
        #define vector nope
        #if __has_include(<vector>)
        YES
        #endif
    ";
    pp!(src, "YES");

    // pas d'erreur d'expansion non plus
    let src = "
        #define F(x, y)
        #if __has_include(<F()>)
        #endif
    ";
    pp!(src, "");

    // ici on ne reconnait pas le header-name donc on expand (GCC fait pareil,
    // mais MSVC et Clang le reconnaissent)
    let src = "
        #define vector nope
        #define P (
        #if __has_include P<vector>)
        YES
        #endif
    ";
    pp!(src, "");

    // ici on reconnait le header-name donc pas d'expansion (GCC/MSVC/Clang aussi)
    let src = "
        #define vector nope
        #define P )
        #if __has_include(<vector>P
        YES
        #endif
    ";
    pp!(src, "YES");

    // si le truc dans __has_include ne correspond pas à un header-name, il faut
    // forcément l'expand
    let src = "
        #define vector nope
        #define V <vector>
        #if __has_include(V)
        YES
        #endif
    ";
    pp!(src, "");

    // header mal formé
    let src = "
        #if __has_include(+)
        #endif
    ";
    diags!(src, [InvalidHeader { error: HeaderError::Malformed, span: span(27..28) }]);
    // tokens en trop
    let src = "
        #if __has_include(<vector> bla)
        #endif
    ";
    diags!(src, [InvalidHeader { error: HeaderError::Malformed, span: span(27..39) }]);
}

#[test]
fn embed() {
    add_file("angle_embeds/data.dat", [1, 2, 3]);
    add_file("angle_embeds/single.dat", [4]);

    // fichiers dans l'embed dir angle
    pp!("#embed <data.dat>", "1,2,3");
    pp!("#embed <single.dat>", "4");
    // accessible aussi avec ""
    pp!("#embed \"data.dat\"", "1,2,3");

    // fichier dans le dossier local
    pp!("#embed \"local.dat\"", "8,3,7,2,4");
    // pas accessible avec <>
    diags!("#embed <local.dat>", [InvalidHeader { error: HeaderError::NotFound("local.dat".into()), span: span(7..18) }]);

    // fichier dans l'embed dir quote
    pp!("#embed \"quote_embed.dat\"", "4,2,1");
    // pas accessible avec <>
    diags!("#embed <quote_embed.dat>", [InvalidHeader { error: HeaderError::NotFound("quote_embed.dat".into()), span: span(7..24) }]);

    // pas besoin que ça soit de l'utf8
    pp!("#embed \"non_utf8.hpp\"", "128");

    // fichier unreadable
    diags!("#embed <unreadable.dat>", [InvalidHeader { error: HeaderError::Unreadable("unreadable.dat".into()), span: span(7..23) }]);

    // headers ajoutés dans les include dir donc pas accessible aux embeds
    diags!("#embed \"quote_include.hpp\"", [InvalidHeader { error: HeaderError::NotFound("quote_include.hpp".into()), span: span(7..26) }]);
    diags!("#embed <quote_include.hpp>", [InvalidHeader { error: HeaderError::NotFound("quote_include.hpp".into()), span: span(7..26) }]);
    diags!("#embed \"angle_include.hpp\"", [InvalidHeader { error: HeaderError::NotFound("angle_include.hpp".into()), span: span(7..26) }]);
    diags!("#embed <angle_include.hpp>", [InvalidHeader { error: HeaderError::NotFound("angle_include.hpp".into()), span: span(7..26) }]);

    // limit
    pp!("#embed <data.dat> limit(0)", "");
    pp!("#embed <data.dat> limit(1)", "1");
    pp!("#embed <data.dat> limit(2)", "1,2");
    pp!("#embed <data.dat> limit(50)", "1,2,3");
    // l'expression se comporte un peu comme dans un #if, si l'identifier n'est pas
    // défini, il est remplacé par 0
    pp!("#embed <data.dat> limit(BLABLA)", "");
    // invalide
    diags!("#embed <data.dat> limit", [ExpectedOperandInParens { operator: pp_kw::Embed, span: span(18..23), has_parens: false }]);
    diags!("#embed <data.dat> limit(", [UnmatchedParenL { span: span(23..24) }]);
    diags!("#embed <data.dat> limit()", [ExpectedOperandInParens { operator: pp_kw::Embed, span: span(23..25), has_parens: true }]);
    diags!("#embed <data.dat> limit(-53)", [NegativeEmbedLimit { span: span(24..27), value: -53 }]);
    diags!("#embed <data.dat> limit(1324889511654865278543999817471356984)", [ParseNumberError::IntValueTooLarge.into_diag("1324889511654865278543999817471356984", span(24..61))]);
    diags!("#embed <data.dat> limit(\"foo\")", [InvalidExpr(vec![ExprError::Str(span(24..29))], pp_kw::Limit)]);
    // defined est interdit
    diags!("#embed <data.dat> limit(defined(FOO))", [DefinedInLimitParam { span: span(24..31) }]);

    // prefix
    pp!("#embed <data.dat> prefix()", "1,2,3");
    pp!("#embed <data.dat> prefix(a,)", "a,1,2,3");
    // il y a un espace entre `42` et `1` car ce sont bien 2 tokens différents,
    // il faut pas les coller
    pp!("#embed <data.dat> prefix(42)", "42 1,2,3");
    // ignoré si empty (soit car vraiment vide ou limit(0))
    pp!("#embed <empty.dat> prefix(42)", "");
    pp!("#embed <data.dat> limit(0) prefix(42)", "");
    // invalide
    diags!("#embed <data.dat> prefix", [ExpectedOperandInParens { operator: pp_kw::Embed, span: span(18..24), has_parens: false }]);
    diags!("#embed <data.dat> prefix(", [UnmatchedParenL { span: span(24..25) }]);

    // suffix
    pp!("#embed <data.dat> suffix()", "1,2,3");
    pp!("#embed <data.dat> suffix(,a)", "1,2,3,a");
    pp!("#embed <data.dat> suffix(42)", "1,2,3 42");
    // ignoré si empty
    pp!("#embed <empty.dat> suffix(42)", "");
    pp!("#embed <data.dat> limit(0) suffix(42)", "");
    // invalide
    diags!("#embed <data.dat> suffix", [ExpectedOperandInParens { operator: pp_kw::Embed, span: span(18..24), has_parens: false }]);
    diags!("#embed <data.dat> suffix(", [UnmatchedParenL { span: span(24..25) }]);

    // if_empty
    pp!("#embed <empty.dat> if_empty()", "");
    pp!("#embed <empty.dat> if_empty(25)", "25");
    pp!("#embed <empty.dat> if_empty(a, b, c)", "a, b, c");
    pp!("#embed <data.dat> limit(0) if_empty(a, b, c)", "a, b, c");
    // invalide
    diags!("#embed <data.dat> if_empty", [ExpectedOperandInParens { operator: pp_kw::Embed, span: span(18..26), has_parens: false }]);
    diags!("#embed <data.dat> if_empty(", [UnmatchedParenL { span: span(26..27) }]);

    // prefix et suffix
    pp!("#embed <data.dat> prefix(0,) suffix(,4)", "0,1,2,3,4");

    // avec expansion
    let src = "
        #define A <data.dat>
        #define L limit(2)
        #embed A L
    ";
    pp!(src, "1,2");
    let src = "
        #define A 2
        #embed <data.dat> limit(A)
    ";
    pp!(src, "1,2");

    // formattage (le calcul de la colonne commence à `embed` et non au `#` donc
    // il y a un espace en trop avant `1`, on pourrait le gérer mais bon on s'en fout)
    let src = "
        [
        #embed <data.dat>
        ]
    ";
    pp!(src, "
        [
         1,2,3
        ]
    ");
    // le préfixe est directement pris de l'endroit original donc il y a plein
    // d'espaces pour rien, on pourrait le gérer (en faisait des substitutions) mais osef
    let src = "
        [
        #embed <data.dat> prefix(0,)
        ]
    ";
    pp!(src, "
        [
                                 0,1,2,3
        ]
    ");

    // incomplet
    diags!("#embed", [ExpectedTokensInDirective { directive: pp_kw::Embed, span: span(1..6) }]);
    // header invalide
    diags!("#embed bla", [InvalidHeader { error: HeaderError::Malformed, span: span(7..10) }]);
    // param invalide
    diags!("#embed <data.dat> +", [ExpectedEmbedParam { span: span(18..19), has_name: true }]);

    // manque les parenthèses pour le param
    diags!("#embed <data.dat> bla", [ExpectedOperandInParens { operator: pp_kw::Embed, span: span(18..21), has_parens: false }]);
    diags!("#embed <data.dat> bla +", [
        ExpectedOperandInParens { operator: pp_kw::Embed, span: span(18..21), has_parens: false },
        ExpectedEmbedParam { span: span(22..23), has_name: true },
    ]);
    diags!("#embed <data.dat> limit(", [UnmatchedParenL { span: span(23..24) }]);
    diags!("#embed <data.dat> bla::blo(", [UnmatchedParenL { span: span(26..27) }]);

    // nom invalide après le préfixe
    diags!("#embed <data.dat> bla::", [ExpectedEmbedParam { span: span(21..23), has_name: false }]);
    diags!("#embed <data.dat> bla::+", [ExpectedEmbedParam { span: span(23..24), has_name: true }]);

    // parenthèses pas obligatoires pour les params préfixés (on a quand même
    // l'erreur unknown car il n'existe aucun attribut préfixé dans le standard donc
    // on peut pas en utiliser un qui marche pour tester les parenthèses optionnelles)
    diags!("#embed <data.dat> bla::blo", [UnknownEmbedParam { prefix: Some(Name::from("bla")), name: Name::from("blo"), span: span(18..26) }]);
    diags!("#embed <data.dat> bla::blo +", [
        ExpectedEmbedParam { span: span(27..28), has_name: true },
        UnknownEmbedParam { prefix: Some(Name::from("bla")), name: Name::from("blo"), span: span(18..26) },
    ]);

    // params inconnus
    diags!("#embed <data.dat> bla() bonjour()", [
        UnknownEmbedParam { prefix: None, name: Name::from("bla"), span: span(18..21) },
        UnknownEmbedParam { prefix: None, name: Name::from("bonjour"), span: span(24..31) },
    ]);

    // params dupliqués
    diags!("#embed <data.dat> limit(0) limit(0)", [DuplicateEmbedParam { name: pp_kw::Limit, old: span(18..23), new: span(27..32) }]);
    diags!("#embed <data.dat> prefix() prefix()", [DuplicateEmbedParam { name: pp_kw::Prefix, old: span(18..24), new: span(27..33) }]);
    diags!("#embed <data.dat> suffix() suffix()", [DuplicateEmbedParam { name: pp_kw::Suffix, old: span(18..24), new: span(27..33) }]);
    diags!("#embed <data.dat> if_empty() if_empty()", [DuplicateEmbedParam { name: pp_kw::IfEmpty, old: span(18..26), new: span(29..37) }]);

    // les `()`, `[]` et `{}` doivent être balanced (les `()` affectent le parsing
    // directement, les autres sont juste checkés après)
    diags!("#embed <data.dat> prefix(()", [UnmatchedParenL { span: span(24..25) }]);
    diags!("#embed <data.dat> prefix())", [ExpectedEmbedParam { span: span(26..27), has_name: true }]);
    diags!("#embed <data.dat> prefix([)", [UnbalancedEmbedParam { kind: TokenKind::BracketL, span: span(25..26), param: pp_kw::Prefix }]);
    diags!("#embed <data.dat> prefix(])", [UnbalancedEmbedParam { kind: TokenKind::BracketR, span: span(25..26), param: pp_kw::Prefix }]);
    diags!("#embed <data.dat> prefix({)", [UnbalancedEmbedParam { kind: TokenKind::BraceL, span: span(25..26), param: pp_kw::Prefix }]);
    diags!("#embed <data.dat> prefix(})", [UnbalancedEmbedParam { kind: TokenKind::BraceR, span: span(25..26), param: pp_kw::Prefix }]);
    diags!("#embed <data.dat> prefix([abc{d]) suffix({ab]c})", [
        UnbalancedEmbedParam { kind: TokenKind::BraceL, span: span(29..30), param: pp_kw::Prefix },
        UnbalancedEmbedParam { kind: TokenKind::BracketR, span: span(44..45), param: pp_kw::Suffix },
    ]);
    // on retourne qu'une seule erreur même si il y a plusieurs trucs unbalanced parce que osef
    diags!("#embed <data.dat> prefix([{)", [UnbalancedEmbedParam { kind: TokenKind::BraceL, span: span(26..27), param: pp_kw::Prefix }]);
    diags!("#embed <data.dat> prefix([abc]{bla])", [UnbalancedEmbedParam { kind: TokenKind::BracketR, span: span(34..35), param: pp_kw::Prefix }]);

    // params formés à partir d'une macro nommée pareil
    let src = "
        #define limit(x) limit(x)
        #define prefix(x) prefix(x)
        #define suffix(x) suffix(x)
        #define if_empty(x) if_empty(x)
        #embed <data.dat> limit(0) prefix() suffix() if_empty()
    ";
    diags!(src, [
        ExpandedStandardEmbedParam { name: pp_kw::Limit, expanded_at: span(173..181), defined_at: Some(span(17..22)) },
        ExpandedStandardEmbedParam { name: pp_kw::Prefix, expanded_at: span(182..190), defined_at: Some(span(51..57)) },
        ExpandedStandardEmbedParam { name: pp_kw::Suffix, expanded_at: span(191..199), defined_at: Some(span(87..93)) },
        ExpandedStandardEmbedParam { name: pp_kw::IfEmpty, expanded_at: span(200..210), defined_at: Some(span(123..131)) },
    ]);
    // erreur même si le param est caché dans une autre macro
    // todo: d'après [cpp.pre] paragraphe 4 ça devrait pas être une erreur parce que
    // l'identifier n'apparaît pas directement dans l'embed mais c'est chiant à
    // gérer pour pas grand chose
    let src = "
        #define A limit(0)
        #define limit(x) limit(x)
        #embed <data.dat> A
    ";
    diags!(src, [ExpandedStandardEmbedParam { name: pp_kw::Limit, expanded_at: span(88..89), defined_at: Some(span(44..49)) }]);
}

#[test]
fn has_embed() {
    // ne peut apparaitre que dans des directives conditionnelles
    diags!("__has_embed", [ForbiddenHasExpr { name: pp_kw::HasEmbed, span: span(0..11) }]);
    // d'après [cpp.cond] (paragraphe 7) il ne devrait même pas apparaître dans
    // un #define (même si les autres compilateurs acceptent)
    diags!("#define A __has_embed", [ForbiddenHasExpr { name: pp_kw::HasEmbed, span: span(10..21) }]);
    // interdit quelque soit la façon dont c'est généré
    let src = "
        #define CC(a, b) a ## b
        CC(__has_em, bed)
    ";
    diags!(src, [ForbiddenHasExpr { name: pp_kw::HasEmbed, span: span(10000019..10000030) }]);

    // on ne peut pas le define ou undef
    diags!("#define __has_embed", [RedefinedPredefMac { name: pp_kw::HasEmbed, span: span(8..19), is_define: true }]);
    diags!("#undef __has_embed", [RedefinedPredefMac { name: pp_kw::HasEmbed, span: span(7..18), is_define: false }]);

    // parenthèses manquantes
    let src = "
        #if __has_embed
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::HasEmbed, span: span(13..24), has_parens: false }]);
    let src = "
        #if __has_embed(
        #endif
    ";
    diags!(src, [UnmatchedParenL { span: span(24..25) }]);
    let src = "
        #if __has_embed()
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::HasEmbed, span: span(24..26), has_parens: true }]);

    // considéré comme une macro définie
    let src = "
        #ifdef __has_embed
        YES
        #endif
    ";
    pp!(src, "YES");

    // include trouvé dans un embed dir angle
    let src = "
        #if __has_embed(<angle_embed.dat>) == __STDC_EMBED_FOUND__
        YES
        #endif
    ";
    pp!(src, "YES");
    // trouvable avec des quotes aussi
    let src = r#"
        #if __has_embed("angle_embed.dat") == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "YES");

    // fichier dans le dossier local
    let src = r#"
        #if __has_embed("local.dat") == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "YES");
    // introuvable avec un header angle
    let src = "
        #if __has_embed(<local.hpp>) == __STDC_EMBED_FOUND__
        YES
        #endif
    ";
    pp!(src, "");

    // pas dans le dossier local mais ajouté comme embed dir quote
    let src = r#"
        #if __has_embed("quote_embed.dat") == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "YES");
    // introuvable avec header angle
    let src = "
        #if __has_embed(<quote_embed.hpp>) == __STDC_EMBED_FOUND__
        YES
        #endif
    ";
    pp!(src, "");

    // on peut embed le fichier non utf8
    let src = r#"
        #if __has_embed("non_utf8.hpp") == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "YES");

    // ill-formed si le fichier n'est pas lisible
    let src = "
        #if __has_embed(<unreadable.dat>)
        #endif
    ";
    diags!(src, [InvalidHeader { error: HeaderError::Unreadable("unreadable.dat".into()), span: span(25..41) }]);

    // fichier inexistant
    let src = r#"
        #if __has_embed(<nope>) == __STDC_EMBED_FOUND__ || __has_embed("nope") == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "");

    // fichiers ajoutés dans un include dir donc pas accessible aux embeds
    let src = r#"
        #if __has_embed("quote_include.hpp") == __STDC_EMBED_FOUND__ || __has_embed(<quote_include.hpp>) == __STDC_EMBED_FOUND__
        YES
        #endif
        #if __has_embed("angle_include.hpp") == __STDC_EMBED_FOUND__ || __has_embed(<angle_include.hpp>) == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "");

    // header mal formé
    let src = "
        #if __has_embed(+)
        #endif
    ";
    diags!(src, [InvalidHeader { error: HeaderError::Malformed, span: span(25..26) }]);

    // avec des params
    let src = r#"
        #if __has_embed("local.dat" limit(3) prefix(+) suffix(+)) == __STDC_EMBED_FOUND__
        YES
        #endif
    "#;
    pp!(src, "YES");

    // params inconnus (pas une erreur)
    let src = r#"
        #if __has_embed("local.dat" bla() bonjour()) == __STDC_EMBED_NOT_FOUND__
        YES
        #endif
    "#;
    pp!(src, "YES");

    // fichier vide
    let src = "
        #if __has_embed(<empty.dat>) == __STDC_EMBED_EMPTY__
        YES
        #endif
    ";
    pp!(src, "YES");
    let src = "
        #if __has_embed(<empty.dat> limit(3)) == __STDC_EMBED_EMPTY__
        YES
        #endif
    ";
    pp!(src, "YES");

    // fichier pas vide mais limit(0)
    let src = r#"
        #if __has_embed("local.dat" limit(0)) == __STDC_EMBED_EMPTY__
        YES
        #endif
    "#;
    pp!(src, "YES");

    // params dupliqués
    diags!("#if __has_embed(\"local.dat\" limit(0) limit(0)) \n #endif", [DuplicateEmbedParam { name: pp_kw::Limit, old: span(28..33), new: span(37..42) }]);
    diags!("#if __has_embed(\"local.dat\" prefix() prefix()) \n #endif", [DuplicateEmbedParam { name: pp_kw::Prefix, old: span(28..34), new: span(37..43) }]);
    diags!("#if __has_embed(\"local.dat\" suffix() suffix()) \n #endif", [DuplicateEmbedParam { name: pp_kw::Suffix, old: span(28..34), new: span(37..43) }]);
    diags!("#if __has_embed(\"local.dat\" if_empty() if_empty()) \n #endif", [DuplicateEmbedParam { name: pp_kw::IfEmpty, old: span(28..36), new: span(39..47) }]);

    // params formés à partir d'une macro nommée pareil
    let src = "
        #define limit(x) limit(x)
        #define prefix(x) prefix(x)
        #define suffix(x) suffix(x)
        #define if_empty(x) if_empty(x)
        #if __has_embed(<data.dat> limit(0) prefix() suffix() if_empty())
        #endif
    ";
    diags!(src, [
        ExpandedStandardEmbedParam { name: pp_kw::Limit, expanded_at: span(182..190), defined_at: Some(span(17..22)) },
        ExpandedStandardEmbedParam { name: pp_kw::Prefix, expanded_at: span(191..199), defined_at: Some(span(51..57)) },
        ExpandedStandardEmbedParam { name: pp_kw::Suffix, expanded_at: span(200..208), defined_at: Some(span(87..93)) },
        ExpandedStandardEmbedParam { name: pp_kw::IfEmpty, expanded_at: span(209..219), defined_at: Some(span(123..131)) },
    ]);
    // erreur même si le param est caché dans une autre macro
    // todo: d'après [cpp.pre] paragraphe 4 ça devrait pas être une erreur parce que
    // l'identifier n'apparaît pas directement dans l'embed mais c'est chiant à
    // gérer pour pas grand chose
    let src = "
        #define A limit(0)
        #define limit(x) limit(x)
        #if __has_embed(<data.dat> A)
        #endif
    ";
    diags!(src, [ExpandedStandardEmbedParam { name: pp_kw::Limit, expanded_at: span(97..98), defined_at: Some(span(44..49)) }]);
}

#[test]
fn has_cpp_attribute() {
    // ne peut apparaitre que dans des directives conditionnelles
    diags!("__has_cpp_attribute", [ForbiddenHasExpr { name: pp_kw::HasCppAttribute, span: span(0..19) }]);
    // si je comprends bien [cpp.cond] (paragraphe 7) il ne devrait même pas
    // apparaître dans un #define (même si les autres compilateurs acceptent)
    diags!("#define A __has_cpp_attribute", [ForbiddenHasExpr { name: pp_kw::HasCppAttribute, span: span(10..29) }]);
    // interdit quelque soit la façon dont c'est généré
    let src = "
        #define CC(a, b) a ## b
        CC(__has_cpp_at, tribute)
    ";
    diags!(src, [ForbiddenHasExpr { name: pp_kw::HasCppAttribute, span: span(10000027..10000046) }]);

    // on ne peut pas le define ou undef
    diags!("#define __has_cpp_attribute", [RedefinedPredefMac { name: pp_kw::HasCppAttribute, span: span(8..27), is_define: true }]);
    diags!("#undef __has_cpp_attribute", [RedefinedPredefMac { name: pp_kw::HasCppAttribute, span: span(7..26), is_define: false }]);

    // parenthèses manquantes
    let src = "
        #if __has_cpp_attribute
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::HasCppAttribute, span: span(13..32), has_parens: false }]);
    let src = "
        #if __has_cpp_attribute(
        #endif
    ";
    diags!(src, [UnmatchedParenL { span: span(32..33) }]);
    let src = "
        #if __has_cpp_attribute()
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::HasCppAttribute, span: span(32..34), has_parens: true }]);

    // considéré comme une macro définie
    let src = "
        #ifdef __has_cpp_attribute
        YES
        #endif
    ";
    pp!(src, "YES");

    // attribut inconnu
    let src = "
        #if __has_cpp_attribute(nope)
        YES
        #endif
        #if __has_cpp_attribute(bla::foo)
        YES
        #endif
    ";
    pp!(src, "");

    // l'argument est expandé
    let src = "
        #define A assume
        #if __has_cpp_attribute(A)
        YES
        #endif
    ";
    pp!(src, "YES");

    // attribut invalide
    let src = "
        #if __has_cpp_attribute(3)
        #endif
    ";
    diags!(src, [InvalidAttr { span: span(33..34) }]);
    let src = "
        #if __has_cpp_attribute(a::b::c)
        #endif
    ";
    diags!(src, [InvalidAttr { span: span(33..40) }]);

    // attributs standards
    pp!("#if __has_cpp_attribute(assume) == 202207L \n            YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(deprecated) == 201309L \n        YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(fallthrough) == 201603L \n       YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(indeterminate) == 202403L \n     YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(likely) == 201803L \n            YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(maybe_unused) == 201603L \n      YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(no_unique_address) == 201803L \n YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(nodiscard) == 201907L \n         YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(noreturn) == 200809L \n          YES \n #endif", "YES");
    pp!("#if __has_cpp_attribute(unlikely) == 201803L \n          YES \n #endif", "YES");
}

#[test]
fn pragma() {
    // les directives inconnues sont réécrites telles quelles pour que le reste
    // du compilateur puisse s'en occuper
    pp!("#pragma", "#pragma");
    pp!("#pragma blabla", "#pragma blabla");
    pp!("#pragma 1 + 2", "#pragma 1 + 2");

    // l'opérateur _Pragma est réécrit en #pragma, ça serait chiant de devoir le
    // réécrire tel quel, GCC/Clang font ça aussi
    let src = r#"
        _Pragma("")
    "#;
    pp!(src, "#pragma");

    let src = r#"
        _Pragma("blabla")
    "#;
    pp!(src, "#pragma blabla");

    let src = r#"
        _Pragma("1 + 2")
    "#;
    pp!(src, "#pragma 1 + 2");

    // les `\"` et `\\` sont remplacés par `"` et `\`
    // (Clang n'affiche pas le \ dans ce cas, mais je crois qu'il faut, peut-être que
    // vu que c'est le dernier \ de la ligne il croit que c'est une line continuation ?)
    let src = r#"
        _Pragma("\"bla\" \\")
    "#;
    pp!(src, r#"
        #pragma "bla" \
    "#);

    // la string peut avoir un préfixe
    let src = r#"
        _Pragma(L"bla")
        _Pragma(u8"bla")
        _Pragma(u"bla")
        _Pragma(U"bla")
    "#;
    pp!(src, "
        #pragma bla
        #pragma bla
        #pragma bla
        #pragma bla
    ");

    // mais pas de suffixe
    // todo: faire un message d'erreur plus précis que "expected string literal"
    let src = r#"
        _Pragma("bla"_abc)
    "#;
    diags!(src, [InvalidPragmaOperand { span: span(17..26) }]);

    // les raw strings sont invalides
    // todo: il faudrait les gérer aussi ?
    let src = r#"
        _Pragma(R"(bla)")
    "#;
    diags!(src, [InvalidPragmaOperand { span: span(17..25) }]);

    // dans une macro
    let src = r#"
        #define A _Pragma("bla")
        A
    "#;
    pp!(src, "#pragma bla");

    // l'expansion se produit avant d'évaluer le pragma
    let src = r#"
        #define A "bla"
        _Pragma(A)
    "#;
    pp!(src, "#pragma bla");

    let src = r#"
        #define PL (
        #define PR )
        #define A "bla"
        _Pragma PL A PR
    "#;
    pp!(src, "#pragma bla");

    let src = r#"
        #define P _Pragma
        #define PL (
        #define PR )
        #define A "bla"
        P PL A PR
    "#;
    pp!(src, "#pragma bla");

    let src = r#"
        #define EMPTY
        #define A "bla" )
        _Pragma(EMPTY A
    "#;
    pp!(src, "#pragma bla");

    // si il y a plusieurs _Pragma sur la même ligne ils sont réécrits un par
    // ligne (sinon ça serait pas considéré comme des directives #pragma)
    let src = r#"
        _Pragma("a") _Pragma("b")
    "#;
    pp!(src, "
#pragma a
#pragma b
    ");

    let src = r#"
        1 _Pragma("a")
    "#;
    pp!(src, "
1
#pragma a
    ");

    // todo: on pourrait mieux gérer les espaces mais bon
    let src = r#"
        #define A 1 _Pragma("a b") 2 _Pragma("c d") 3
        A
    "#;
    pp!(src, "
 1
#pragma a b
 2
#pragma c d
 3
    ");

    // pas un pragma (car pas au début de la ligne) donc ça reste sur la même ligne
    pp!("1 #pragma a", "1 #pragma a");

    // erreurs
    diags!("_Pragma", [ExpectedOperandInParens { operator: pp_kw::PragmaOp, span: span(0..7), has_parens: false }]);
    diags!("_Pragma(", [UnmatchedParenL { span: span(7..8) }]);
    diags!("_Pragma()", [ExpectedOperandInParens { operator: pp_kw::PragmaOp, span: span(7..9), has_parens: true }]);
    diags!("_Pragma(bla)", [InvalidPragmaOperand { span: span(8..11) }]);

    // on n'attend pas de trouver la parenthèse fermante pour reconnaître que
    // l'opérande est invalide, sinon on avancerait jusqu'à trouver une parenthèse
    // fermante peut-être 300 lignes plus bas qui a rien à voir avec le pragma,
    // tout ça pour se retrouver avec l'erreur qui dit que l'opérande est invalide,
    // en soulignant 300 lignes...
    diags!("_Pragma(1", [InvalidPragmaOperand { span: span(8..9) }]);

    let src = "
        _Pragma(1 + 2
        )
    ";
    diags!(src, [InvalidPragmaOperand { span: span(17..18) }]);
    let src = "
        _Pragma(1 + 2
        bla )
    ";
    diags!(src, [InvalidPragmaOperand { span: span(17..18) }]);

    // mais tant qu'on reste sur la même ligne on se donne quand même l'opportunité
    // d'avancer jusqu'à la parenthèse fermante, pour pouvoir souligner toute
    // l'opérande invalide et pas juste le premier token
    diags!("_Pragma(1 + 2)", [InvalidPragmaOperand { span: span(8..13) }]);

    // pas géré dans une directive (comme #if)
    // todo: meilleure erreur ?
    let src = r#"
        #if _Pragma("bla")
        #endif
    "#;
    diags!(src, [InvalidExpr(vec![
        ExprError::Str(span(21..26)),
        ExprError::InvalidUnOp(span(13..20), UnOpKind::Other),
    ], kw::If)]);

    // on vérifie que c'est le formattage est bon
    let src = r#"
        1
        _Pragma
        (
        "bla"
        )
        2
    "#;
    pp!(src, "
        1
        #pragma bla
        2
    ");

    let src = "
        1
        #pragma bla
        2
    ";
    pp!(src, "
        1
        #pragma bla
        2
    ");
}

#[test]
fn pragma_once() {
    let opts = || PpOptions {
        pragma_once: true,
        ..Default::default()
    };

    add_file("src/pragma_once.hpp", "
        #pragma once
        hello
    ");
    let src = r#"
        #include "pragma_once.hpp"
        #include "pragma_once.hpp"
    "#;
    pp!(opts(), src, "
        hello
    ");

    // extra tokens
    diags!(opts(), "#pragma once bla bla", [TokensAfterDirective { spans: vec!(span(13..16), span(17..20)) }]);
    // todo: les tokens bla bla sont dans une source virtuelle, c'est pas ouf
    // de pointer dessus car du coup on voit pas d'où ils viennent au niveau du
    // code original
    diags!(opts(), r#"_Pragma("once bla bla")"#, [TokensAfterDirective { spans: vec!(span(10000005..10000008), span(10000009..10000012)) }]);

    // pas d'effet si désactivé
    add_file("src/no_pragma_once.hpp", "
        #pragma once
        hello
    ");
    let src = r#"
        #include "no_pragma_once.hpp"
        #include "no_pragma_once.hpp"
        #pragma once bla bla
    "#;
    pp!(src, "
        #pragma once
        hello
        #pragma once
        hello
        #pragma once bla bla
    ");
}

#[test]
fn defined() {
    let src = "
        #if defined(__cplusplus)
        YES
        #endif
    ";
    pp!(src, "YES");

    let src = "
        #if defined __cplusplus
        YES
        #endif
    ";
    pp!(src, "YES");

    let src = "
        #if (defined __cplusplus)
        YES
        #endif
    ";
    pp!(src, "YES");

    // pas défini
    let src = "
        #if defined(BLA)
        YES
        #endif
    ";
    pp!(src, "");

    let src = "
        #define A
        #if defined A + B
        YES
        #endif
    ";
    pp!(src, "YES");

    // defined ne peut apparaitre que directement, pas après une expansion
    let src = "
        #define DEFINED defined
        #if DEFINED(FOO)
        #endif
    ";
    diags!(src, [DefinedAppearedAfterExpansion { span: span(45..52) }]);

    // opérande invalide
    let src = "
        #if defined
        #endif
    ";
    diags!(src, [InvalidDefinedOperand { span: span(13..20), has_operand: false, has_parens: false }]);
    let src = "
        #if defined(
        #endif
    ";
    diags!(src, [UnmatchedParenL { span: span(20..21) }]);
    let src = "
        #if defined()
        #endif
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::Defined, span: span(20..22), has_parens: true }]);
    let src = "
        #if defined +
        #endif
    ";
    diags!(src, [InvalidDefinedOperand { span: span(21..22), has_operand: true, has_parens: false }]);
    let src = "
        #if defined 2
        #endif
    ";
    diags!(src, [InvalidDefinedOperand { span: span(21..22), has_operand: true, has_parens: false }]);
    let src = "
        #if defined(2)
        #endif
    ";
    diags!(src, [InvalidDefinedOperand { span: span(21..22), has_operand: true, has_parens: true }]);
    let src = "
        #if defined(A + B)
        #endif
    ";
    diags!(src, [InvalidDefinedOperand { span: span(21..26), has_operand: true, has_parens: true }]);

    // avec une comma
    let src = "
        #if defined A, B
        #endif
    ";
    diags!(src, [InvalidExpr(vec![ExprError::InvalidBinOp(span(22..23), BinOpKind::Comma)], kw::If)]);
    let src = "
        #if defined(A), B
        #endif
    ";
    diags!(src, [InvalidExpr(vec![ExprError::InvalidBinOp(span(23..24), BinOpKind::Comma)], kw::If)]);
    let src = "
        #if (defined A), B
        #endif
    ";
    diags!(src, [InvalidExpr(vec![ExprError::InvalidBinOp(span(24..25), BinOpKind::Comma)], kw::If)]);
    let src = "
        #if defined(A, B)
        #endif
    ";
    diags!(src, [InvalidDefinedOperand { span: span(21..25), has_operand: true, has_parens: true }]);

    // todo: ce cas devrait être accepté ?
    let src = "
        #define B 1
        #if (defined A, B)
        YES
        #endif
    ";
    diags!(src, [InvalidExpr(vec![ExprError::InvalidBinOp(span(43..44), BinOpKind::Comma)], kw::If)]);
}

#[test]
fn error() {
    let src = "
        #error salut il y a une erreur
    ";
    diags!(src, [ErrorWarningDirective { is_warn: false, span: span(10..15), message: "salut il y a une erreur".into() }]);

    // vide
    let src = "
        #error
    ";
    diags!(src, [ErrorWarningDirective { is_warn: false, span: span(10..15), message: "".into() }]);

    // les macros ne sont pas expand
    let src = "
        #define A 42
        #error A
    ";
    diags!(src, [ErrorWarningDirective { is_warn: false, span: span(31..36), message: "A".into() }]);

    // avec line continuations
    let src = r"
        #error blabla \
            + 2 \
            - 1
    ";
    diags!(src, [ErrorWarningDirective { is_warn: false, span: span(10..15), message: "blabla + 2 - 1".into() }]);

    // commentaire multiline
    // todo: ça serait mieux que ça aille pas à ligne dans le message ?
    let src = "
        #error blabla /*
        */ + 2
    ";
    diags!(src, [ErrorWarningDirective { is_warn: false, span: span(10..15), message: "blabla\n           + 2".into() }]);

    // erreur si il y a des tokens invalides
    // c'est idiot parce que si on veut écrire un message qui contient un `'`
    // il croit que c'est un unterminated char mais bon c'est censé être des
    // tokens valides (GCC fait pareil, Clang et MSVC ignorent les erreurs)
    let src = "
        #error ''
    ";
    diags!(src, [
        ErrorWarningDirective { is_warn: false, span: span(10..15), message: "''".into() },
        LexError::Char(CharError::Empty, span(16..18)),
    ]);
}

#[test]
fn warning() {
    let src = "
        #warning salut ceci est un warning
    ";
    diags!(src, [ErrorWarningDirective { is_warn: true, span: span(10..17), message: "salut ceci est un warning".into() }]);

    // vide
    let src = "
        #warning
    ";
    diags!(src, [ErrorWarningDirective { is_warn: true, span: span(10..17), message: "".into() }]);

    // les macros ne sont pas expand
    let src = "
        #define A 42
        #warning A
    ";
    diags!(src, [ErrorWarningDirective { is_warn: true, span: span(31..38), message: "A".into() }]);

    // avec line continuations
    let src = r"
        #warning blabla \
            + 2 \
            - 1
    ";
    diags!(src, [ErrorWarningDirective { is_warn: true, span: span(10..17), message: "blabla + 2 - 1".into() }]);

    // commentaire multiline
    let src = "
        #warning blabla /*
        */ + 2
    ";
    diags!(src, [ErrorWarningDirective { is_warn: true, span: span(10..17), message: "blabla\n           + 2".into() }]);

    // erreur si il y a des tokens invalides
    let src = "
        #warning ''
    ";
    diags!(src, [
        ErrorWarningDirective { is_warn: true, span: span(10..17), message: "''".into() },
        LexError::Char(CharError::Empty, span(18..20)),
    ]);
}

#[test]
fn line() {
    let main_path = full_path("src/main.cpp");

    let src = "
        a
        __LINE__
    ";
    pp!(src, "
        a
        3
    ");

    // dans une macro
    let src = "
        #define F G
        #define G __LINE__
        F
        G
    ";
    pp!(src, "
        4
        5
    ");

    // dans un arg
    let src = "
        #define F(x) x
        F(__LINE__)
    ";
    pp!(src, "3");

    // dans une macro et un arg
    let src = "
        #define F() G(__LINE__)
        #define G(x) x
        F()
    ";
    pp!(src, "4");

    // dans une macro sur plusieurs lignes
    let src = r"
        #define F 1 \
            + \
            __LINE__
        F
    ";
    pp!(src, "1 + 5");

    // dans des args sur plusieurs lignes
    // GCC/Clang affichent `4 5` comme ici, MSVC affiche `6 6`, EDG affiche `4 6`
    let src = "
        #define F(x, y) x y
        F(
            __LINE__,
            __LINE__
        )
    ";
    pp!(src, "4 5");

    // pareil mais dans une macro, la line est celle de G
    let src = r"
        #define F(x, y) x y
        #define G F( \
            __LINE__, \
            __LINE__ \
        )
        G
    ";
    pp!(src, "7 7");

    // invocation sur plusieurs lignes, __LINE__ prend la valeur de la fin du span
    // (parenthèse fermante)
    // c'est un choix arbitraire je sais pas ce qu'il vaut mieux, MSVC/Clang/EDG
    // font pareil et GCC affiche 3
    let src = "
        #define F() __LINE__
        F(

        )
    ";
    pp!(src, "5");

    // avec #line
    let src = "
        __LINE__
        #line 42
        __LINE__
        __LINE__


        __LINE__
    ";
    pp!(src, "
        2

        42
        43


        46
    ");

    let src = "
        #line 42


        __LINE__
    ";
    pp!(src, "
        44
    ");

    let src = "
        #line 1
        __LINE__
        #line 2147483647
        __LINE__
        __LINE__
        __LINE__
    ";
    pp!(src, "
        1

        2147483647
        2147483648
        2147483649
    ");

    let src = "
        #line 43
        __LINE__
        #line 2
        __LINE__
    ";
    pp!(src, "
        43

        2
    ");

    // peut contenir des '
    let src = "
        #line 69'482
        __LINE__
    ";
    pp!(src, "69482");

    let src = "
        #line __LINE__
        __LINE__
    ";
    pp!(src, "2");

    // les line continuations comptent comme des newlines
    let src = r"
        salut \
        bonjour \
        2 + 2
        __LINE__
    ";
    pp!(src, "
        salut bonjour 2 + 2
        5
    ");

    // si c'est un nombre octal on l'interprète comme un nombre décimal (+ warning)
    let src = "
        #line 069
        __LINE__
        #line 00042
        __LINE__
    ";
    pp_and_diags!(src, "
        69

        42
    ", [
        OctalNumberInLineDirective { span: span(15..18), value: 69 },
        OctalNumberInLineDirective { span: span(50..55), value: 42 },
    ]);

    // incomplet
    diags!("#line", [ExpectedTokensInDirective { directive: pp_kw::Line, span: span(1..5) }]);

    // __FILE__
    let src = r#"
        __FILE__
        #line 1 "blabla.cpp"
        __FILE__
        #line 1 "toto"
        __FILE__
    "#;
    pp!(src, format!(r#"
        "{main_path}"

        "blabla.cpp"

        "toto"
    "#));

    // si il y a une directive qui introduit un filename, il est gardé si les
    // nouvelles directives n'ont pas de filename
    let src = r#"
        #line 1 "salut"
        __FILE__
        #line 1
        __FILE__
        __FILE__
    "#;
    pp!(src, r#"
        "salut"

        "salut"
        "salut"
    "#);

    // filename vide
    let src = r#"
        #line 1 ""
        __FILE__
    "#;
    pp!(src, r#"
        ""
    "#);

    // le filename n'est pas réécrit "tel quel", si il contient des escape sequences
    // elles auront déjà été traitées mais les newlines, `\"` et `\\` sont escapés
    let src = r#"
        #line 1 "bla\nbla"
        __FILE__
        #line 1 "bla\rbla"
        __FILE__
        #line 1 "bla\r\nbla"
        __FILE__
        #line 1 "bla\\bla"
        __FILE__
        #line 1 "bla\"bla"
        __FILE__
        #line 1 "bla\tbla"
        __FILE__
        #line 1 "bla\abla"
        __FILE__
    "#;
    pp!(src, r#"
        "bla\nbla"

        "bla\nbla"

        "bla\nbla"

        "bla\\bla"

        "bla\"bla"

        "bla	bla"

        "blabla"
    "#);

    // line continuation dans la directive
    let src = r#"
        #line \
            42
        __LINE__
    "#;
    pp!(src, "42");

    let src = r#"
        #line \
            42 \
            "bla"
        __LINE__ __FILE__
    "#;
    pp!(src, r#"
        42 "bla"
    "#);

    // avec expansion
    let src = r#"
        #define A 42
        #define B "bla"
        #line A B
        __LINE__ __FILE__
    "#;
    pp!(src, r#"
        42 "bla"
    "#);

    let src = r#"
        #define A 42 "bla"
        #line A
        __LINE__ __FILE__
    "#;
    pp!(src, r#"
        42 "bla"
    "#);

    let src = r#"
        #define A "bla"
        #line 42 A
        __LINE__ __FILE__
    "#;
    pp!(src, r#"
        42 "bla"
    "#);

    // pas obligé que le filename représente de l'utf8 valide
    let src = r#"
        #line 1 "\257"
        __FILE__
    "#;
    pp!(src, r#"
        "�"
    "#);

    // pas un nombre
    diags!("#line \"salut\"", [InvalidLineNumber { kind: InvalidLineNumberKind::NotANumber, span: span(6..13) }]);
    diags!("#line abc", [InvalidLineNumber { kind: InvalidLineNumberKind::NotANumber, span: span(6..9) }]);
    diags!("#line 4.2", [InvalidLineNumber { kind: InvalidLineNumberKind::NotANumber, span: span(6..9) }]);
    diags!("#line -76", [InvalidLineNumber { kind: InvalidLineNumberKind::NotANumber, span: span(6..7) }]);
    diags!("#line (23)", [InvalidLineNumber { kind: InvalidLineNumberKind::NotANumber, span: span(6..7) }]);
    // nombre invalide
    diags!("#line 0b01", [InvalidLineNumber { kind: InvalidLineNumberKind::InvalidDigits, span: span(6..10) }]);
    diags!("#line 0x35", [InvalidLineNumber { kind: InvalidLineNumberKind::InvalidDigits, span: span(6..10) }]);
    diags!("#line 35u", [InvalidLineNumber { kind: InvalidLineNumberKind::InvalidDigits, span: span(6..9) }]);
    diags!("#line 35_abc", [InvalidLineNumber { kind: InvalidLineNumberKind::InvalidDigits, span: span(6..12) }]);
    // nombre out of range
    diags!("#line 0", [InvalidLineNumber { kind: InvalidLineNumberKind::OutOfRange, span: span(6..7) }]);
    diags!("#line 2147483648", [InvalidLineNumber { kind: InvalidLineNumberKind::OutOfRange, span: span(6..16) }]);
    diags!("#line 95843584553698446556448968545468998449849898449846498984948849498465456", [InvalidLineNumber { kind: InvalidLineNumberKind::OutOfRange, span: span(6..77) }]);

    // octal + out of range, on affiche bien les 2 diags
    diags!("#line 0213654762314", [
        OctalNumberInLineDirective { span: span(6..19), value: 213654762314 },
        InvalidLineNumber { kind: InvalidLineNumberKind::OutOfRange, span: span(6..19) },
    ]);
    // mais pas si il est vraiment trop grand
    // todo: ça serait mieux d'afficher le warning octal aussi
    diags!("#line 0213654762235674567512345461235673456743254217634547314", [
        InvalidLineNumber { kind: InvalidLineNumberKind::OutOfRange, span: span(6..61) },
    ]);

    // filename invalide
    diags!("#line 1 4", [InvalidLineFileName { span: span(8..9) }]);
    diags!("#line 1 +", [InvalidLineFileName { span: span(8..9) }]);
    diags!("#line 1 abc", [InvalidLineFileName { span: span(8..11) }]);
    // prefix et suffix pas autorisés
    diags!("#line 1 u8\"salut\"", [InvalidLineFileName { span: span(8..17) }]);
    diags!("#line 1 \"salut\"_abc", [InvalidLineFileName { span: span(8..19) }]);
    // il me semble que les raw string literals sont interdits même si les autres
    // compilateurs acceptent
    diags!("#line 1 R\"(salut)\"", [InvalidLineFileName { span: span(8..18) }]);

    // extra tokens
    diags!("#line 1 \"salut\" 3 + 4", [TokensAfterDirective { spans: vec![span(16..17), span(18..19), span(20..21)] }]);
    // les string literals ne sont pas encore concaténés à ce stade donc erreur
    diags!("#line 1 \"salut\" \"abc\"", [TokensAfterDirective { spans: vec![span(16..21)] }]);
    // filename invalide + extra tokens
    diags!("#line 1 2 3", [
        InvalidLineFileName { span: span(8..9) },
        TokensAfterDirective { spans: vec![span(10..11)] },
    ]);

    // espaces
    let src = "
        ; __LINE__
        ;__LINE__
    ";
    pp!(src, "
        ; 2
        ;3
    ");

    let src = "
        ; __FILE__
        ;__FILE__
    ";
    pp!(src, format!(r#"
        ; "{main_path}"
        ;"{main_path}"
    "#));

    // dans un fichier inclut
    add_file("src/line_file.hpp", "


        __LINE__
        __FILE__
    ");
    let line_file_path = full_path("src/line_file.hpp");
    let src = r#"
        #include "line_file.hpp"
    "#;
    pp!(src, format!(r#"
        4
        "{line_file_path}"
    "#));
}

#[test]
fn define() {
    // incomplet
    diags!("#define", [ExpectedTokensInDirective { directive: pp_kw::Define, span: span(1..7) }]);

    // #define dans un body de macro (pas un vrai define)
    let src = "
        #define DEFINE #define
        DEFINE A 1 + 2
        A
    ";
    pp!(src, "
        #define A 1 + 2
        A
    ");

    // espaces après le #
    let src = "
        #        define A 42
        A
    ";
    pp!(src, "42");

    // doit être le premier token de la ligne pour être une directive
    let src = "
        #define EMPTY
        EMPTY #define A 42
        A
    ";
    pp!(src, "
        #define A 42
        A
    ");

    // on ne peut pas redefine une macro prédéfinie
    diags!("#define __cplusplus", [RedefinedPredefMac { name: Name::from("__cplusplus"), span: span(8..19), is_define: true }]);
    diags!("#define __LINE__", [RedefinedPredefMac { name: Name::from("__LINE__"), span: span(8..16), is_define: true }]);

    // invalid macro name
    diags!("#define 5", [InvalidMacName { lexeme: "5".into(), span: span(8..9), is_name: false }]);
    diags!("#define +", [InvalidMacName { lexeme: "+".into(), span: span(8..9), is_name: false }]);

    // un alternative token ne peut pas être un nom de macro (car pas un identifiant)
    diags!("#define and", [InvalidMacName { lexeme: "and".into(), span: span(8..11), is_name: false }]);
    diags!("#define or", [InvalidMacName { lexeme: "or".into(), span: span(8..10), is_name: false }]);
    diags!("#define xor", [InvalidMacName { lexeme: "xor".into(), span: span(8..11), is_name: false }]);
    diags!("#define not", [InvalidMacName { lexeme: "not".into(), span: span(8..11), is_name: false }]);
    diags!("#define bitand", [InvalidMacName { lexeme: "bitand".into(), span: span(8..14), is_name: false }]);
    diags!("#define bitor", [InvalidMacName { lexeme: "bitor".into(), span: span(8..13), is_name: false }]);
    diags!("#define compl", [InvalidMacName { lexeme: "compl".into(), span: span(8..13), is_name: false }]);
    diags!("#define and_eq", [InvalidMacName { lexeme: "and_eq".into(), span: span(8..14), is_name: false }]);
    diags!("#define or_eq", [InvalidMacName { lexeme: "or_eq".into(), span: span(8..13), is_name: false }]);
    diags!("#define xor_eq", [InvalidMacName { lexeme: "xor_eq".into(), span: span(8..14), is_name: false }]);
    diags!("#define not_eq", [InvalidMacName { lexeme: "not_eq".into(), span: span(8..14), is_name: false }]);
    diags!("#define %:", [InvalidMacName { lexeme: "%:".into(), span: span(8..10), is_name: false }]);
    diags!("#define %:%:", [InvalidMacName { lexeme: "%:%:".into(), span: span(8..12), is_name: false }]);
    diags!("#define <%", [InvalidMacName { lexeme: "<%".into(), span: span(8..10), is_name: false }]);
    diags!("#define %>", [InvalidMacName { lexeme: "%>".into(), span: span(8..10), is_name: false }]);
    diags!("#define <:", [InvalidMacName { lexeme: "<:".into(), span: span(8..10), is_name: false }]);
    diags!("#define :>", [InvalidMacName { lexeme: ":>".into(), span: span(8..10), is_name: false }]);

    diags!("#define defined", [InvalidMacName { lexeme: "defined".into(), span: span(8..15), is_name: true }]);

    // ne peut pas être un keyword non plus (pas la peine de tout tester parce que osef)
    diags!("#define struct", [InvalidMacName { lexeme: "struct".into(), span: span(8..14), is_name: true }]);
    // ni un contextual keyword
    diags!("#define final", [InvalidMacName { lexeme: "final".into(), span: span(8..13), is_name: true }]);
    // ni un attribut
    diags!("#define assume", [InvalidMacName { lexeme: "assume".into(), span: span(8..14), is_name: true }]);

    // mais likely/unlikely est autorisé pour une fn-like macro
    diags!("#define likely", [InvalidMacName { lexeme: "likely".into(), span: span(8..14), is_name: true }]);
    diags!("#define unlikely", [InvalidMacName { lexeme: "unlikely".into(), span: span(8..16), is_name: true }]);
    pp!("#define likely()", "");
    pp!("#define unlikely()", "");
}

#[test]
fn obj_macro() {
    let src = "
        #define A 3
        A
    ";
    pp!(src, "3");

    let src = "
        #define A 3
        A + NotAMacro
    ";
    pp!(src, "3 + NotAMacro");

    let src = "
        #define A 1 + 2
        3 + A - 4 - A
    ";
    pp!(src, "3 + 1 + 2 - 4 - 1 + 2");

    // empty body
    let src = "
        #define A
        1 + A
    ";
    pp!(src, "1 +");

    // body in parentheses (not a function)
    let src = "
        #define A (1 + 2)
        #define B/**/(1 + 2)
        A B
    ";
    pp!(src, "(1 + 2) (1 + 2)");

    // pas d'expansion récursive
    let src = "
        #define A 1 + A
        A
    ";
    pp!(src, "1 + A");

    let src = "
        #define A 1 + B
        #define B 2 + A
        A B
    ";
    pp!(src, "1 + 2 + A 2 + 1 + B");

    // pas un appel
    let src = "
        #define A 1
        A(42)
    ";
    pp!(src, "1(42)");

    // erreur si pas d'espace entre le nom et le premier token
    diags!("#define A+", [NoSpaceAfterMacName { name: Name::from("A"), first_span: span(9..10) }]);

    // redéfinition
    let src = "
        #define A 1 + 2
        A
        #define A   1 /* blabla */    + /**/ 2
        A
    ";
    pp!(src, "
        1 + 2

        1 + 2
    ");

    // avec body vide
    let src = "
        #define A
        A
        #define A
        A
    ";
    pp!(src, "");

    // erreur si pas le même body
    let src = "
        #define A 1 + 2
        #define A 1 + 3
    ";
    diags!(src, [MacRedefined { name: Name::from("A"), old: Some(span(17..18)), new: span(41..42) }]);
    let src = "
        #define A 1 + 2
        #define A 1 - 2
    ";
    diags!(src, [MacRedefined { name: Name::from("A"), old: Some(span(17..18)), new: span(41..42) }]);
    let src = "
        #define A 1 + 2
        #define A 1 + 2 + 3
    ";
    diags!(src, [MacRedefined { name: Name::from("A"), old: Some(span(17..18)), new: span(41..42) }]);

    // les espaces doivent aussi être cohérents
    let src = "
        #define A 1 + 2
        #define A 1 +2
    ";
    diags!(src, [MacRedefined { name: Name::from("A"), old: Some(span(17..18)), new: span(41..42) }]);
}

#[test]
fn fn_macro() {
    let src = "
        #define F() 1 + 2
        F()
    ";
    pp!(src, "1 + 2");

    let src = "
        #define F() (1 + 2)
        F()
    ";
    pp!(src, "(1 + 2)");

    let src = "
        #define F() G()
        #define G() 1 + 2
        F()
    ";
    pp!(src, "1 + 2");

    // on peut mettre des espaces / newlines avant la liste d'arguments
    let src = "
        #define F() 1
        F   () F
        ()
    ";
    pp!(src, "1 1");

    // récursivité
    let src = "
        #define A() B()
        #define B() A()
        #define X A() B()
        A() B()
        X
    ";
    pp!(src, "
        A() B()
        A() B()
    ");

    let src = "
        #define A() B
        #define B() A
        #define X A() A()() A()()()
        A() A()() A()()()
        X
    ";
    pp!(src, "
        B A B
        B A B
    ");

    // G ne peut plus jamais se faire expand même si il n'apparaissait pas sous
    // la forme d'un appel (dans F)
    let src = "
        #define F() G
        #define G() F()
        #define EXPAND(x) x
        G()()
        EXPAND(G()())
    ";
    pp!(src, "
        G()
        G()
    ");

    let src = "
        #define F(x) x
        #define X F(F(F(1)))
        F(F(F(1)))
        X
    ";
    pp!(src, "
        1
        1
    ");

    // args avec le même nom sont bien expandés
    let src = "
        #define F(x, y) x + y
        #define A 1
        F(A, A)
    ";
    pp!(src, "1 + 1");

    // pas des appels de fonction
    let src = "
        #define A 1
        #define F()
        F A
    ";
    pp!(src, "F 1");

    let src = "
        #define A 1
        #define B F A
        #define F()
        B
    ";
    pp!(src, "F 1");

    let src = "
        #define F() 1
        (F)()
    ";
    pp!(src, "(F)()");

    let src = "
        #define F() 1
        #define P (
        #define EXPAND(x) x
        F P)
        EXPAND(F P))
    ";
    pp!(src, "
        F ()
        1
    ");

    // avec args
    let src = "
        #define F(a) a
        F(42)
    ";
    pp!(src, "42");

    let src = "
        #define F(a, b) a + b + a
        F(42, 69)
    ";
    pp!(src, "42 + 69 + 42");

    // variadics
    let src = "
        #define F(...) __VA_ARGS__
        +F() F(1) F(2, 3) F(4, 5, 6)+
    ";
    pp!(src, "+ 1 2, 3 4, 5, 6+");

    let src = "
        #define F(a, ...) __VA_ARGS__ a
        +F() F(1) F(2, 3) F(4, 5, 6)+
    ";
    pp!(src, "+ 1 3 2 5, 6 4+");

    // les args variadics ne sont pas expandés si __VA_ARGS__ n'apparaît pas
    // dans un contexte où il devrait être expandé (donc pas d'erreur sur le
    // manque d'args pour G)
    let src = "
        #define F(...) 1
        #define G(x, y)
        F(G())
    ";
    pp!(src, "1");

    // les args variadics sont expandés si __VA_OPT__ est utilisé (donc erreur)
    let src = "
        #define F(...) __VA_OPT__(1)
        #define G(x, y)
        F(G())
    ";
    diags!(src, [WrongMacNumArgs { expected: 2, actual: 1, variadic: false, defined_at: Some(span(54..55)), args_span: span(10000015..10000017), name: Name::from("G") }]);

    // args vides
    let src = "
        #define F(a) a
        +F();
    ";
    pp!(src, "+;");

    let src = "
        #define F(a, b) a + b
        F(, 2) F(1, )
    ";
    pp!(src, "+ 2 1 +");

    let src = "
        #define F(...) __VA_ARGS__
        F(,) F(1, ) F(2, )
    ";
    pp!(src, ", 1, 2,");

    let src = "
        #define F(a, ...) __VA_ARGS__ a
        +F(,) F(1, ) F(, 2)+
    ";
    pp!(src, "+ 1 2+");

    // pas d'expansion récursive même à l'intérieur d'un __VA_OPT__
    let src = "
        #define F(...) __VA_OPT__(F(__VA_ARGS__))
        #define EXPAND(x) x
        F(42)
        EXPAND(F(42))
    ";
    pp!(src, "
        F(42)
        F(42)
    ");

    // newline dans un __VA_OPT__
    let src = "
        #define F(...) __VA_OPT__(1 +
            2)
    ";
    diags!(src, [UnmatchedParenL { span: span(34..35) }]);

    // unterminated
    diags!("#define F(...) __VA_OPT__", [ExpectedOperandInParens { operator: pp_kw::VaOpt, span: span(15..25), has_parens: false }]);
    diags!("#define F(...) __VA_OPT__(", [UnmatchedParenL { span: span(25..26) }]);
    diags!("#define F(...) __VA_OPT__(()", [UnmatchedParenL { span: span(25..26) }]);

    // va_opt dans va_opt
    diags!("#define F(...) __VA_OPT__(1 + __VA_OPT__(2))", [NestedVaOpt { span: span(30..40) }]);
    diags!("#define F(...) __VA_OPT__(1 + __VA_OPT__)", [NestedVaOpt { span: span(30..40) }]);

    // même en cas d'erreur on ajoute quand même la macro à la table et on l'expand
    // mais en ignorant le va_opt erroné
    // de toute façon on est pas censé voir la sortie du préprocesseur en cas d'erreur
    // mais c'est bien que ça soit ajouté quand même pour par ex avoir une erreur
    // en cas de redéfinition puisque la macro existe bien, c'est juste qu'elle
    // a un body invalide (MSVC fait comme ça aussi, Clang et GCC ignorent la première
    // définition)
    let src = "
        #define F(...) 1 + __VA_OPT__
        F(bla)
        #define F 2
        F
    ";
    pp_and_diags!(src, "
        1 +

        F
    ", [
        ExpectedOperandInParens { operator: pp_kw::VaOpt, span: span(28..38), has_parens: false },
        MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(70..71) },
    ]);

    // parenthèses imbriquées
    let src = "
        #define F(...) __VA_OPT__((1, 2))
        #define G(...) __VA_OPT__((1, (2, 3)))
        F(1)
        G(1)
    ";
    pp!(src, "
        (1, 2)
        (1, (2, 3))
    ");

    // les tokens dans un __VA_OPT__ ne sont pas expandés au préalable
    let src = "
        #define F(...) __VA_OPT__(A) ## 2
        #define G(...) #__VA_OPT__(A)
        #define A 1
        F(_)
        G(_)
    ";
    pp!(src, r#"
        A2
        "A"
    "#);

    // mais on utilise la version expandé des arguments
    let src = "
        #define F(...) __VA_OPT__(__VA_ARGS__) ## 2
        #define G(...) #__VA_OPT__(__VA_ARGS__)
        #define A 1
        F(A)
        G(A)
    ";
    pp!(src, r#"
        12
        "1"
    "#);
    let src = "
        #define F(a, ...) __VA_OPT__(a) ## 2
        #define G(a, ...) #__VA_OPT__(a)
        #define A 1
        F(A, _)
        G(A, _)
    ";
    pp!(src, r#"
        12
        "1"
    "#);

    // sauf si ils apparaissent dans un # ou ##
    let src = "
        #define F(...) __VA_OPT__(__VA_ARGS__ ## 2)
        #define G(...) __VA_OPT__(2 ## __VA_ARGS__)
        #define H(...) __VA_OPT__(1 ## __VA_ARGS__ ## 2)
        #define I(...) __VA_OPT__(#__VA_ARGS__)
        #define A 1
        F(A)
        G(A)
        H(A)
        I(A)
    ";
    pp!(src, r#"
        A2
        2A
        1A2
        "A"
    "#);
    let src = "
        #define F(a, ...) __VA_OPT__(a ## 2)
        #define G(a, ...) __VA_OPT__(2 ## a)
        #define H(a, ...) __VA_OPT__(1 ## a ## 2)
        #define I(a, ...) __VA_OPT__(#a)
        #define A 1
        F(A, _)
        G(A, _)
        H(A, _)
        I(A, _)
    ";
    pp!(src, r#"
        A2
        2A
        1A2
        "A"
    "#);

    // si il y a plusieurs placemarkers dans le va_opt, on en garde qu'un, je
    // suis pas 100% sûr mais je crois que c'est ce qu'il faut faire
    // (Clang génère `12` comme ici, GCC/MSVC génèrent `1 2`)
    let src = "
        #define F(x, y, ...) 1 ## __VA_OPT__(x y) ## 2
        F(, , _)
    ";
    pp!(src, "12");

    // le contenu de va_opt est utilisé uniquement si les va_args ne sont pas
    // vide (après expansion)
    let src = "
        #define F(...) __VA_OPT__(42)
        #define EMPTY
        F() F(EMPTY) F(a)
    ";
    pp!(src, "42");

    // body vide
    let src = "
        #define F()
        #define G(a, b, c)
        1 + F()
        1 + G(1, 2, 3)
    ";
    pp!(src, "
        1 +
        1 +
    ");

    // redéfinition
    let src = "
        #define F(a) a + 2
        F(1)
        #define F( a   )    a /*  */    +   2
        F(1)
    ";
    pp!(src, "
        1 + 2

        1 + 2
    ");

    // avec body vide
    let src = "
        #define F()
        F()
        #define F()
        F()
    ";
    pp!(src, "");

    // erreur si pas le même body
    let src = "
        #define F(a) a + 2
        #define F(a) a + 3
    ";
    diags!(src, [MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(44..45) }]);
    let src = "
        #define F(a) a + 2
        #define F(a) a - 2
    ";
    diags!(src, [MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(44..45) }]);
    let src = "
        #define F(a) a + 2
        #define F(a) a + 2 + 3
    ";
    diags!(src, [MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(44..45) }]);

    // erreur si pas le même type de macro
    let src = "
        #define A 1
        #define A() 1
    ";
    diags!(src, [MacRedefined { name: Name::from("A"), old: Some(span(17..18)), new: span(37..38) }]);

    // erreur si pas le même nombre de params
    let src = "
        #define F(a) 1
        #define F(a, b) 1
    ";
    diags!(src, [MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(40..41) }]);

    // erreur si pas les params n'ont pas le même nom
    let src = "
        #define F(a) 1
        #define F(b) 1
    ";
    diags!(src, [MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(40..41) }]);

    // il faut aussi que les espaces soient cohérents
    let src = "
        #define F(a) a + 2
        #define F(a) a +2
    ";
    diags!(src, [MacRedefined { name: Name::from("F"), old: Some(span(17..18)), new: span(44..45) }]);

    // mais l'espace avant le premier token ne compte pas
    let src = "
        #define F(a)   a + 2
        F(1)
        #define F(a)a + 2
        F(1)
    ";
    pp!(src, "
        1 + 2

        1 + 2
    ");

    // le paramètre est une fn-like macro
    let src = "
        #define F(f) f(3)
        #define G(x) x
        F(G)
    ";
    pp!(src, "3");

    // appel de macro avec des arguments à moitié d'un côté et de l'autre
    let src = "
        #define A B(1,
        #define B F
        #define F(a, b) a + b
        A 2)
    ";
    pp!(src, "1 + 2");

    // ça n'appelle pas F car la parenthèse provient de l'expansion de A
    let src = "
        #define A (1,
        #define B F
        #define F(a, b) a b
        F A 2)
    ";
    pp!(src, "F (1, 2)");

    // test extrait de https://stackoverflow.com/questions/3136686/is-the-c99-preprocessor-turing-complete/79690813#79690813
    // MSVC/GCC/Clang crashent car ils expandent indéfiniment, EDG donne le même
    // résultat qu'ici
    let src = "
        #define g(x)f((h(x))
        #define h(x)f((g(x))
        #define f(x)x
        g(x))
    ";
    pp!(src, "((g(x))");

    // ici le F qui apparaît au cours de l'expansion de G ne se fait pas expand
    // car il est encore dans le contexte du premier F, c'est un peu comme le cas
    // précédent
    // ce n'est pas clair si c'est ça qu'il faut faire, dans ce cas ça change pas
    // grand chose mais dans le cas précédent ça part en récursion infinie donc
    // je pense que c'est mieux de faire comme ça
    // MSVC/GCC/Clang font l'expansion (EDG fait comme ici, pas d'expansion)
    let src = "
        #define F(h) h(G()
        #define G() F()
        #define H(x) x
        F(H))
    ";
    pp!(src, "F()");

    // un exemple plus complexe, il y a deux G qui apparaissent dans les args.
    // le premier (comme dans l'exemple précédent) ne se fait pas expand,
    // pour les mêmes raisons, mais le deuxième est en dehors du contexte donc
    // lui se fait bien expand
    let src = "
        #define F(h) h(G(),
        #define G() F()
        #define H(x, y) x + y
        F(H) G())
    ";
    pp!(src, "F() + (G(),");

    // pareil mais avec x qui n'apparaît pas dans le body de H
    let src = "
        #define F(h) h(G(),
        #define G() F()
        #define H(x, y) y
        F(H) G())
    ";
    pp!(src, "(G(),");

    // pareil mais avec la virgule en dehors de F
    let src = "
        #define F(h) h(G()
        #define G() F()
        #define H(x, y) x + y
        F(H), G())
    ";
    pp!(src, "F() + (G()");

    // autre exemple comme le précédent sauf que les deux G apparaissent dans le
    // même argument mais c'est pareil, le premier G est dans le contexte du F donc
    // pas d'expansion mais le 2ème oui
    let src = "
        #define F(h) h(G()
        #define G() F()
        #define H(x) x
        F(H) G())
    ";
    pp!(src, "F() (G()");

    // pareil que les 2 exemples précédents mais on appelle directement F au lieu
    // de G
    let src = "
        #define F(h) h(G()
        #define G() F()
        #define H(x) x
        F(H) F())
    ";
    pp!(src, "F() (F()");
    let src = "
        #define F(h) h(G(),
        #define G() F()
        #define H(x, y) x + y
        F(H) F())
    ";
    pp!(src, "F() + (F(),");

    // les arguments sont expandés séparément donc il faut que les appels de macro
    // soient complets dans l'argument lui même
    let src = "
        #define F(x) x, 2)
        #define G(x, y) x + y
        #define A G(1
        F(A)
    ";
    diags!(src, [UnterminatedMacCall { name: Name::from("G"), span: span(10000008..10000009) }]);

    // pas le bon nombre d'arguments
    let src = "
        #define F() 1
        F(1) F(,)
    ";
    diags!(src, [
        WrongMacNumArgs { expected: 0, actual: 1, variadic: false, defined_at: Some(span(17..18)), args_span: span(32..35), name: Name::from("F") },
        WrongMacNumArgs { expected: 0, actual: 2, variadic: false, defined_at: Some(span(17..18)), args_span: span(37..40), name: Name::from("F") },
    ]);

    let src = "
        #define F(a) a
        F(,)
    ";
    diags!(src, [WrongMacNumArgs { expected: 1, actual: 2, variadic: false, defined_at: Some(span(17..18)), args_span: span(33..36), name: Name::from("F") }]);

    let src = "
        #define F(a, b) a b
        F(1) F(1, 2, 3) F(1, 2, ,)
    ";
    diags!(src, [
        WrongMacNumArgs { expected: 2, actual: 1, variadic: false, defined_at: Some(span(17..18)), args_span: span(38..41), name: Name::from("F") },
        WrongMacNumArgs { expected: 2, actual: 3, variadic: false, defined_at: Some(span(17..18)), args_span: span(43..52), name: Name::from("F") },
        WrongMacNumArgs { expected: 2, actual: 4, variadic: false, defined_at: Some(span(17..18)), args_span: span(54..63), name: Name::from("F") },
    ]);

    let src = "
        #define F(a, b, ...) __VA_ARGS__
        F(1)
    ";
    // 2 actual car la macro est variadique donc on ajoute un va arg explicitement
    diags!(src, [WrongMacNumArgs { expected: 3, actual: 2, variadic: true, defined_at: Some(span(17..18)), args_span: span(51..54), name: Name::from("F") }]);

    // plusieurs tokens dans un arg
    let src = "
        #define F(a, b) a + b
        F(1 * 2 / 3, 4 * 5)
    ";
    pp!(src, "1 * 2 / 3 + 4 * 5");

    // les virgules dans des parenthèses internes ne comptent pas comme séparateurs
    // d'arguments
    let src = "
        #define F(a) a
        F((1, 2, 3, 4, 5))
    ";
    pp!(src, "(1, 2, 3, 4, 5)");

    let src = "
        #define F(a) a
        F(((1, 2, (3, 4, (5, 6)))))
    ";
    pp!(src, "((1, 2, (3, 4, (5, 6))))");

    let src = "
        #define F(a, b) a b
        F(1, (2, 3))
        F((1, 2), 3)
        F(1 + (2), 3)
    ";
    pp!(src, "
        1 (2, 3)
        (1, 2) 3
        1 + (2) 3
    ");

    // un arg inutilisé ou utilisé dans un stringize ou concat n'est pas expandé
    // (donc pas d'erreur sur le manque d'args pour X)
    let src = "
        #define F(a) 1
        #define G(a) 1 ## a
        #define H(a) #a
        #define X(a, b)
        F(X())
        G(X())
        H(X())
    ";
    pp!(src, r#"
        1
        1X()
        "X()"
    "#);

    // on a pas le droit de mettre des directives dans la liste d'arguments
    let src = "
        #define F(a) a
        F(
  #if false
            42
  #else
            69
  #endif
        )
    ";
    diags!(src, [
        DirectiveInMacArgs { span: span(37..38) },
        DirectiveInMacArgs { span: span(64..65) },
        DirectiveInMacArgs { span: span(87..88) },
    ]);

    // ok, ce ne sont pas des directives car pas au début de la ligne
    let src = "
        #define F(a) a
        F(
    abc #if false
            42
    def #else
            69
    ghi #endif
        )
    ";
    pp!(src, "abc #if false 42 def #else 69 ghi #endif");

    let src = "
        #define F(a) a
        F(#if false 42 #else 69 #endif)
    ";
    pp!(src, "#if false 42 #else 69 #endif");

    // __VA_ARGS__ et __VA_OPT__ ne peuvent apparaître que dans un body de macro
    // variadique
    // (pas d'erreur sur le fait que __VA_OPT__ n'a pas d'argument car il est pas
    // censé être là en premier lieu)
    diags!("#define A __VA_ARGS__ + __VA_OPT__", [
        LexError::ForbiddenVaArgs(pp_kw::VaArgs, span(10..21)),
        LexError::ForbiddenVaArgs(pp_kw::VaOpt, span(24..34)),
    ]);
    diags!("#define F() __VA_ARGS__ + __VA_OPT__", [
        LexError::ForbiddenVaArgs(pp_kw::VaArgs, span(12..23)),
        LexError::ForbiddenVaArgs(pp_kw::VaOpt, span(26..36)),
    ]);
    diags!("#define F(a) __VA_ARGS__ + __VA_OPT__", [
        LexError::ForbiddenVaArgs(pp_kw::VaArgs, span(13..24)),
        LexError::ForbiddenVaArgs(pp_kw::VaOpt, span(27..37)),
    ]);
    diags!("#define __VA_ARGS__ 42", [LexError::ForbiddenVaArgs(pp_kw::VaArgs, span(8..19))]);
    diags!("#define __VA_OPT__ 42", [LexError::ForbiddenVaArgs(pp_kw::VaOpt, span(8..18))]);

    // liste de paramètres invalide
    // ellipsis
    diags!("#define F(a, ..., b)", [InvalidMacParamList(vec![MacParamListError::EllipsisNotAtEnd(span(13..16))])]);
    // duplicate param
    diags!("#define F(a, a)", [InvalidMacParamList(vec![MacParamListError::DuplicateParam(Name::from("a"), span(13..14))])]);
    // newline
    diags!("#define F(\n)", [InvalidMacParamList(vec![MacParamListError::HasNewline(span(8..9))])]);
    diags!("#define F(a\n)", [InvalidMacParamList(vec![MacParamListError::HasNewline(span(8..9))])]);
    diags!("#define F(a,\n)", [InvalidMacParamList(vec![MacParamListError::HasNewline(span(8..9))])]);
    diags!("#define F(", [InvalidMacParamList(vec![MacParamListError::HasNewline(span(8..9))])]);
    diags!("#define F(a,", [InvalidMacParamList(vec![MacParamListError::HasNewline(span(8..9))])]);
    // expected name
    diags!("#define F(+)", [InvalidMacParamList(vec![MacParamListError::ExpectedName(span(10..11))])]);
    diags!("#define F(,)", [InvalidMacParamList(vec![MacParamListError::ExpectedName(span(10..11))])]);
    diags!("#define F(a,)", [InvalidMacParamList(vec![MacParamListError::ExpectedName(span(12..13))])]);
    diags!("#define F(a, (b), c)", [InvalidMacParamList(vec![MacParamListError::ExpectedName(span(13..14))])]);
    // expected comma
    diags!("#define F(a+)", [InvalidMacParamList(vec![MacParamListError::ExpectedComma(span(11..12))])]);
    diags!("#define F(a, b-)", [InvalidMacParamList(vec![MacParamListError::ExpectedComma(span(14..15))])]);

    // unterminated call
    let src = "
        #define F()
        F(
    ";
    diags!(src, [UnterminatedMacCall { name: Name::from("F"), span: span(29..30) }]);

    let src = "
        #define F(x, y) x + y
        F(1,
    ";
    diags!(src, [UnterminatedMacCall { name: Name::from("F"), span: span(39..40) }]);

    // la parenthèse fermante est rattachée au second appel, donc elle ne compte
    // pas pour le premier
    let src = "
        #define F(x, y) x + y
        F(1,
        F(1, 2)
    ";
    diags!(src, [UnterminatedMacCall { name: Name::from("F"), span: span(39..40) }]);

    // on ne peut pas commencer à appeler une macro dans un fichier et finir dans un autre
    add_file("src/macro_call.hpp", "
        #define F() 1
        F(
    ");
    let src = r#"
        #include "macro_call.hpp"
        )
    "#;
    diags!(src, [UnterminatedMacCall { name: Name::from("F"), span: span(10000070..10000071) }]);
}

#[test]
fn undef() {
    // incomplet
    diags!("#undef", [ExpectedTokensInDirective { directive: pp_kw::Undef, span: span(1..6) }]);

    let src = "
        #define A 42
        A
        #undef A
        A
    ";
    pp!(src, "
        42

        A
    ");

    let src = "
        #define F(a) a
        F(1)
        #undef F
        F(1)
    ";
    pp!(src, "
        1

        F(1)
    ");

    // #undef d'une macro non définie ne fait rien
    let src = "
        #undef TOTO
        42
    ";
    pp!(src, "42");

    // on ne peut pas undef une macro prédéfinie
    diags!("#undef __cplusplus", [RedefinedPredefMac { name: Name::from("__cplusplus"), span: span(7..18), is_define: false }]);
    diags!("#undef __LINE__", [RedefinedPredefMac { name: Name::from("__LINE__"), span: span(7..15), is_define: false }]);

    // invalid macro name
    diags!("#undef 5", [InvalidMacName { lexeme: "5".into(), span: span(7..8), is_name: false }]);
    diags!("#undef +", [InvalidMacName { lexeme: "+".into(), span: span(7..8), is_name: false }]);

    // defined est invalide
    diags!("#undef defined", [InvalidMacName { lexeme: "defined".into(), span: span(7..14), is_name: true }]);

    // ne peut pas être un keyword non plus (on teste pas tout parce que osef)
    diags!("#undef struct", [InvalidMacName { lexeme: "struct".into(), span: span(7..13), is_name: true }]);
    // ni un contextual keyword
    diags!("#undef final", [InvalidMacName { lexeme: "final".into(), span: span(7..12), is_name: true }]);
    // ni un attribut
    diags!("#undef assume", [InvalidMacName { lexeme: "assume".into(), span: span(7..13), is_name: true }]);

    // mais likely/unlikely est autorisé
    pp!("#undef likely", "");
    pp!("#undef unlikely", "");
}

#[test]
fn null_directive() {
    let src = "
        #
        salut
    ";
    pp!(src, "salut");

    // avec une directive juste après
    let src = "
        #
        #define A 1
        A
    ";
    pp!(src, "1");

    // pas une null directive car pas au début de la ligne
    let src = "
        salut #
    ";
    pp!(src, "salut #");
}

#[test]
fn invalid_directive() {
    diags!("#blabla", [InvalidDirective { is_name: true, span: span(1..7) }]);
    diags!("#+", [InvalidDirective { is_name: false, span: span(1..2) }]);
    diags!("# 5", [InvalidDirective { is_name: false, span: span(2..3) }]);
    diags!("# 'b'", [InvalidDirective { is_name: false, span: span(2..5) }]);
}

#[test]
fn stringize() {
    let src = "
        #define F(x) #x
        F(1)
        F(1 + 2)
    ";
    pp!(src, r#"
        "1"
        "1 + 2"
    "#);

    // avec espace
    let src = "
        #define F(x) # x
        F(42)
    ";
    pp!(src, r#"
        "42"
    "#);

    // l'argument n'est pas expandé
    let src = "
        #define F(a) #a
        #define A 42
        F(A)
    ";
    pp!(src, r#"
        "A"
    "#);

    // doit être suivi d'un paramètre
    let src = "
        #define F(x) #a
    ";
    diags!(src, [HashNotFollowedByParam { span: span(22..23) }]);
    let src = "
        #define A 42
        #define F(x) #A
    ";
    diags!(src, [HashNotFollowedByParam { span: span(43..44) }]);
    let src = "
        #define F(x) #+
    ";
    diags!(src, [HashNotFollowedByParam { span: span(22..23) }]);
    let src = "
        #define F() #
    ";
    diags!(src, [HashNotFollowedByParam { span: span(21..22) }]);

    // un va_args ou va_opt en dehors d'une macro variadique est déjà invalide
    // donc pas besoin de rajouter l'erreur que c'est pas suivi d'un param, vu
    // que ça serait un param valide dans une macro variadique
    let src = "
        #define F() #__VA_ARGS__
    ";
    diags!(src, [LexError::ForbiddenVaArgs(pp_kw::VaArgs, span(22..33))]);
    let src = "
        #define F() #__VA_OPT__(1)
    ";
    diags!(src, [LexError::ForbiddenVaArgs(pp_kw::VaOpt, span(22..32))]);

    // arg vide
    let src = r#"
        #define F(a, b) #a + #b
        #define G(a) #a
        F(abc, )
        F(, def)
        G()
    "#;
    pp!(src, r#"
        "abc" + ""
        "" + "def"
        ""
    "#);

    // stringize un char ou str
    let src = r#"
        #define F(x) #x
        F('a')
        F('\n')
        F("blabla")
        F("bla\nbla")
    "#;
    pp!(src, r#"
        "'a'"
        "'\\n'"
        "\"blabla\""
        "\"bla\\nbla\""
    "#);

    // un newline (dans une raw string) est remplacé par \n
    let src = r#"
        #define F(x) #x
        F(R"(salut
            bonjour)")
    "#;
    pp!(src, r#"
        "R\"(salut\n            bonjour)\""
    "#);

    // les espaces avant et après les arguments sont ignorés et une suite d'espaces
    // ou newlines ou commentaires est considéré comme un espace
    let src = "
        #define F(x) #x
        F(    abc  /* blablabla */  /*
        */  def
                // bonjour

ghi    )
    ";
    pp!(src, r#"
        "abc def ghi"
    "#);

    // escapes sequences (en dehors d'un char / str)
    let src = r"
        #define F(x) #x
        F(\n)
        F(\0)
        F(\o{123})
        F(\xFF)
    ";
    pp!(src, r#"
        "\n"
        "\0"
        "\o{123}"
        "\xFF"
    "#);

    // caratères non ascii
    let src = "
        #define F(x) #x
        F(😀)
        F(éà)
    ";
    pp!(src, r#"
        "😀"
        "éà"
    "#);

    // contrairement à la sortie formattée du préprocesseur, on n'essaie pas
    // dans un stringize d'éviter de concaténer des tokens qui étaient distincts
    let src = "
        #define STR(x) #x
        #define STRX(x) STR(x)
        #define A +
        STRX(A+ +A ++)
    ";
    pp!(src, r#"
        "++ ++ ++"
    "#);

    // erreur si ça forme une chaîne invalide
    // chaîne non terminée ("\")
    let src = r"
        #define F(x) #x
        F(\)
    ";
    diags!(src, [InvalidStringize { stringize_span: span(10000000..10000002), arg_span: Some(span(35..36)), lexeme: r#""\""#.into() }]);
    // chaîne contenant des escapes invalides
    let src = r"
        #define F(x) #x
        F(abc \ def)
        F(\xFFFF)
        F(\o{})
    ";
    diags!(src, [
        InvalidStringize { stringize_span: span(10000000..10000002), arg_span: Some(span(35..44)), lexeme: r#""abc \ def""#.into() },
        InvalidStringize { stringize_span: span(10000015..10000017), arg_span: Some(span(56..62)), lexeme: r#""\xFFFF""#.into() },
        InvalidStringize { stringize_span: span(10000027..10000029), arg_span: Some(span(74..78)), lexeme: r#""\o{}""#.into() },
    ]);
    // invalide dans un va_opt
    // todo: on n'affiche pas le span de l'argument mais il faudrait d'une manière
    // ou d'une autre
    let src = r"
        #define F(...) #__VA_OPT__(__VA_ARGS__)
        F(\)
    ";
    diags!(src, [InvalidStringize { stringize_span: span(10000000..10000011), arg_span: None, lexeme: r#""\""#.into() }]);

    // variadic
    let src = "
        #define F(...) #__VA_ARGS__
        F() F(1) F(1, 2)
    ";
    pp!(src, r#"
        "" "1" "1, 2"
    "#);

    // va_args n'est pas expandé
    let src = "
        #define F(...) #__VA_ARGS__
        #define A 42
        F(A)
    ";
    pp!(src, r#"
        "A"
    "#);

    // va_args vide qui apparait dans un stringize
    let src = "
        #define F(x) #x
        #define G(...) F(1;__VA_ARGS__;2)
        G()
    ";
    pp!(src, r#"
        "1;;2"
    "#);

    // va_opt vide
    let src = "
        #define F(x) #x
        #define G(...) F(1;__VA_OPT__(__VA_ARGS__);2)
        G()
    ";
    pp!(src, r#"
        "1;;2"
    "#);

    // stringize va_opt
    let src = "
        #define F(...) #__VA_OPT__(1)
        F()
        F(_)
    ";
    pp!(src, r#"
        ""
        "1"
    "#);
    let src = "
        #define F(...) #__VA_OPT__()
        F()
        F(_)
    ";
    pp!(src, r#"
        ""
        ""
    "#);

    // plusieurs tokens dans le va_opt
    let src = "
        #define F(...) #__VA_OPT__(1 2 3)
        F(_)
    ";
    pp!(src, r#"
        "1 2 3"
    "#);

    // stringize un truc stringizé
    let src = "
        #define F(...) #__VA_OPT__(#__VA_ARGS__)
        F(1)
    ";
    pp!(src, r#"
        "\"1\""
    "#);

    // erreur si # non suivi d'un param dans un va_opt
    diags!("#define F(...) __VA_OPT__(#) __VA_ARGS__", [HashNotFollowedByParam { span: span(26..27) }]);

    // si le # provient d'une macro ou d'un param, ce n'est pas l'opérateur #
    let src = "
        #define HASH #
        #define F(x) HASH x
        #define EXPAND(x) x
        F(1)
        EXPAND(F(1))
    ";
    pp!(src, "
        # 1
        # 1
    ");
    let src = "
        #define F(x, hash) hash x
        #define EXPAND(x) x
        F(1, #)
        EXPAND(F(1, #))
        F(x, #)
        EXPAND(F(x, #))
    ";
    pp!(src, "
        # 1
        # 1
        # x
        # x
    ");

    // on ne peut pas ajouter de préfixe
    let src = "
        #define F(x) L#x
        F(salut)
    ";
    pp!(src, "L \"salut\"");
}

#[test]
fn concat() {
    let src = "
        #define A 1 ## 2
        #define F() 1 ## 2
        A F()
    ";
    pp!(src, "12 12");

    // avec / sans espaces
    let src = "
        #define A 1##2
        #define F() 1##2
        A F()
    ";
    pp!(src, "12 12");
    let src = "
        #define A 1## 2
        #define F() 1## 2
        A F()
    ";
    pp!(src, "12 12");
    let src = "
        #define A 1 ##2
        #define F() 1 ##2
        A F()
    ";
    pp!(src, "12 12");

    // autres tokens avant / après
    let src = "
        #define A 42 1 ## 2
        #define F() 42 1 ## 2
        A F()
    ";
    pp!(src, "42 12 42 12");
    let src = "
        #define A 1 ## 2 42
        #define F() 1 ## 2 42
        A F()
    ";
    pp!(src, "12 42 12 42");
    let src = "
        #define A 69 1 ## 2 42
        #define F() 69 1 ## 2 42
        A F()
    ";
    pp!(src, "69 12 42 69 12 42");

    // plusieurs concats consécutifs
    let src = "
        #define A 1 ## 2 ## 3
        #define F() 1 ## 2 ## 3
        A F()
    ";
    pp!(src, "123 123");
    let src = "
        #define A 1 ## 2 3 ## 4
        #define F() 1 ## 2 3 ## 4
        A F()
    ";
    pp!(src, "12 34 12 34");

    // un token concaténé peut continuer à se faire expand
    let src = "
        #define A F ## OO
        #define F() F ## OO
        #define CONCAT(a, b) a ## b
        #define FOO 42
        A F() CONCAT(F, OO)
    ";
    pp!(src, "42 42 42");

    // peut former un UCN
    let src = r"
        #define A \##u00E9
        #define F() \##u00E9
        A F()
    ";
    pp!(src, "é é");

    // mais on ne peut pas former un UCN dans ce cas car le lexer râle dès qu'il voit `\u`
    // et considère que c'est un UCN non terminé (donc erreur)
    // todo: il faudrait supporter ce cas ?
    let src = r"
        #define A \u ## 00E9
        #define F() \u ## 00E9
        A F()
    ";
    diags!(src, [
        LexError::Escape(EscapeError::ExpectedHexDigits(4), span(19..20)),
        LexError::Escape(EscapeError::ExpectedHexDigits(4), span(50..51)),
    ]);

    // le concat est prioritaire sur l'appel
    let src = r"
        #define F() A ## B()
        #define AB() 42
        #define B() 1
        F()
    ";
    pp!(src, "42");

    // ## ne peut pas être au début ou fin d'une replacement list ou __VA_OPT__
    let src = "
        #define A 1 ##
        #define F() 1 ##
        #define G(...) __VA_OPT__(1 ##) 2
    ";
    diags!(src, [
        HashHashAtStartOrEnd { span: span(21..23), at_start: false, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(46..48), at_start: false, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(85..87), at_start: false, in_va_opt: true },
    ]);
    let src = "
        #define A ## 2
        #define F() ## 2
        #define G(...) 1 __VA_OPT__(## 2)
    ";
    diags!(src, [
        HashHashAtStartOrEnd { span: span(19..21), at_start: true, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(44..46), at_start: true, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(85..87), at_start: true, in_va_opt: true },
    ]);
    let src = "
        #define A ##
        #define F() ##
        #define G(...) 1 __VA_OPT__(##) 2
    ";
    diags!(src, [
        HashHashAtStartOrEnd { span: span(19..21), at_start: true, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(42..44), at_start: true, in_va_opt: false },
        // todo: ça serait bien qu'il n'y ait pas 2 erreurs pour le même ##
        HashHashAtStartOrEnd { span: span(81..83), at_start: true, in_va_opt: true },
        HashHashAtStartOrEnd { span: span(81..83), at_start: false, in_va_opt: true },
    ]);
    let src = "
        #define A ## 1 ## 2 ##
        #define F() ## 1 ## 2 ##
        #define G(...) 3 __VA_OPT__(## 1 ## 2 ##) 4
    ";
    diags!(src, [
        HashHashAtStartOrEnd { span: span(19..21), at_start: true, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(29..31), at_start: false, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(52..54), at_start: true, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(62..64), at_start: false, in_va_opt: false },
        HashHashAtStartOrEnd { span: span(101..103), at_start: true, in_va_opt: true },
        HashHashAtStartOrEnd { span: span(111..113), at_start: false, in_va_opt: true },
    ]);

    // si le va_opt est invalide (pas de parenthèses), pas d'erreur sur le ##
    let src = "
        #define F(...) __VA_OPT__ ## 2
    ";
    diags!(src, [ExpectedOperandInParens { operator: pp_kw::VaOpt, span: span(24..34), has_parens: false }]);

    // erreur si la concaténation ne forme pas exactement un et un seul token
    let src = "
        #define A 1 ## +
        #define F() 1 ## +
        A F()
    ";
    diags!(src, [
        InvalidConcat { lhs_lexeme: "1", rhs_lexeme: "+", hash_hash_span: span(10000002..10000004) },
        InvalidConcat { lhs_lexeme: "1", rhs_lexeme: "+", hash_hash_span: span(10000012..10000014) },
    ]);
    let src = "
        #define A 1 ## ## 2
        #define F() 1 ## ## 2
        A F()
    ";
    diags!(src, [
        InvalidConcat { lhs_lexeme: "1", rhs_lexeme: "##", hash_hash_span: span(10000002..10000004) },
        InvalidConcat { lhs_lexeme: "1", rhs_lexeme: "##", hash_hash_span: span(10000016..10000018) },
    ]);

    // on peut pas non plus former des commentaires
    let src = "
        #define A / ## /
        A
    ";
    diags!(src, [InvalidConcat { lhs_lexeme: "/", rhs_lexeme: "/", hash_hash_span: span(10000002..10000004) }]);
    let src = "
        #define A / ## *
        A
    ";
    diags!(src, [InvalidConcat { lhs_lexeme: "/", rhs_lexeme: "*", hash_hash_span: span(10000002..10000004) }]);

    // les arguments ne sont pas expandés dans le cadre d'un concat
    let src = "
        #define F(a, b, c) a ## b ## c a b c
        #define A 1
        #define B 2
        #define C 3
        F(A, B, C)
    ";
    pp!(src, "ABC 1 2 3");

    // ici le concat est un "niveau" plus bas donc les args sont bien expandés
    let src = "
        #define CC(a, b) a ## b
        #define F(a, b) CC(a, b) a b
        #define A 42
        #define B 69
        F(A, B)
    ";
    pp!(src, "4269 42 69");

    // arguments vides (tokens placemarkers)
    let src = "
        #define F(x, y, z) x ## y ## z
        F(1,2,3) F(,,) F(,4,5) F(6,,7) F(8,9,) F(10,,) F(,11,) F(,,12)
    ";
    pp!(src, "123 45 67 89 10 11 12");

    // après concaténation, si le token nomme un paramètre, il n'est pas remplacé
    // par l'argument
    let src = "
        #define F(a, b, foo) a ## b foo
        F(f, oo, 42)
    ";
    pp!(src, "foo 42");

    // variadic
    let src = "
        #define F(...) 1 ## __VA_ARGS__
        ;F() F(2, 3);
    ";
    pp!(src, ";1 12, 3;");
    let src = "
        #define F(...) __VA_ARGS__ ## 1
        ;F() F(2, 3);
    ";
    pp!(src, ";1 2, 31;");
    let src = "
        #define F(...) 1 ## __VA_ARGS__ ## 2
        ;F() F(3, 4);
    ";
    pp!(src, ";12 13, 42;");

    // va_opt
    let src = "
        #define F(...) 1 ## __VA_OPT__(__VA_ARGS__)
        ;F() F(2, 3);
    ";
    pp!(src, ";1 12, 3;");
    let src = "
        #define F(...) __VA_OPT__(__VA_ARGS__) ## 1
        ;F() F(2, 3);
    ";
    pp!(src, ";1 2, 31;");
    let src = "
        #define F(...) 1 ## __VA_OPT__(__VA_ARGS__) ## 2
        ;F() F(3, 4);
    ";
    pp!(src, ";12 13, 42;");

    let src = "
        #define F(...) 1 ## __VA_OPT__(abc)
        F(_)
    ";
    pp!(src, "1abc");
    let src = "
        #define F(...) __VA_OPT__(abc) ## 1
        F(_)
    ";
    pp!(src, "abc1");
    let src = "
        #define F(...) 1 ## __VA_OPT__(abc) ## 2
        F(_)
    ";
    pp!(src, "1abc2");

    // les concat dans un va_opt sont faits avant les autres (donc erreur)
    let src = "
        #define F(...) 1 ## e ## __VA_OPT__(+ ## 2)
        F(_)
    ";
    diags!(src, [InvalidConcat { lhs_lexeme: "+", rhs_lexeme: "2", hash_hash_span: span(10000023..10000025) }]);
    // l'ordre n'est pas spécifié mais ici c'est fait de gauche à droite donc
    // sans le va_opt c'est bien valide à chaque étape (1e -> 1e+ -> 1e+2)
    let src = "
        #define F() 1 ## e ## + ## 2
        F()
    ";
    pp!(src, "1e+2");

    // plusieurs tokens dans le va_opt
    let src = "
        #define F(...) __VA_OPT__(2 3 4) ## 5
        #define G(...) 1 ## __VA_OPT__(2 3 4)
        #define H(...) 1 ## __VA_OPT__(2 3 4) ## 5
        F(_)
        G(_)
        H(_)
    ";
    pp!(src, "
        2 3 45
        12 3 4
        12 3 45
    ");

    // la concaténation ne se produit pas en dehors d'un body de macro
    pp!("1 ## 2", "1 ## 2");

    let src = "
        #define F(x) x
        F(1 ## 2)
    ";
    pp!(src, "1 ## 2");

    let src = "
        #define A F(1 ## 2)
        #define F(x) x
        A
    ";
    pp!(src, "12");

    // ## ne fonctionne pas si il est formé par concaténation ou qu'il provient
    // d'un paramètre
    let src = "
        #define HASH_HASH # ## #
        #define EXPAND(x) x
        #define A 1 HASH_HASH 2
        #define F(hash_hash) 1 hash_hash 2
        A EXPAND(A)
        F(##) EXPAND(F(##))
    ";
    pp!(src, "
        1 ## 2 1 ## 2
        1 ## 2 1 ## 2
    ");
    // les arguments se font bien expand (car ce n'est pas un vrai ##)
    let src = "
        #define HASH_HASH # ## #
        #define F1(a, b) a HASH_HASH b
        #define F2(a, b, hash_hash) a hash_hash b
        #define A 42
        #define B 69
        F1(A, B) F2(A, B, ##)
    ";
    pp!(src, "42 ## 69 42 ## 69");
    // si ## se retrouve au début ou à la fin ce n'est pas une erreur car ce
    // n'est pas un vrai ##
    let src = "
        #define F(x) x
        #define EXPAND(x) x
        F(## 1 ## 2 ##)
        EXPAND(F(## 1 ## 2 ##))
    ";
    pp!(src, "
        ## 1 ## 2 ##
        ## 1 ## 2 ##
    ");

    // on ne peut pas former un ## par concaténation dans une fn-like macro
    // (car considéré comme le stringizing operator)
    let src = "
        #define F() # ## #
    ";
    diags!(src, [
        HashNotFollowedByParam { span: span(21..22) },
        HashNotFollowedByParam { span: span(26..27) },
    ]);

    // ce n'est pas un comportement standard de faire disparaître la virgule
    // dans ce cas donc on ne le fait pas
    let src = "
        #define F(...) , ## __VA_ARGS__
        #define G(x, ...) , ## __VA_ARGS__
        F()
        G(0)
    ";
    pp!(src, "
        ,
        ,
    ");

    // et c'est une erreur d'essayer de concaténer `,` et `1` même si les
    // compilateurs acceptent en tant qu'extension non standard
    let src = "
        #define F(...) , ## __VA_ARGS__
        F(1, 2, 3)
    ";
    diags!(src, [InvalidConcat { lhs_lexeme: ",", rhs_lexeme: "1", hash_hash_span: span(10000002..10000004) }]);
}

#[test]
fn concat_and_stringize() {
    // l'ordre d'évaluation entre # et ## n'est pas spécifié, dans notre cas on
    // évalue le # en premier
    let src = "
        #define F(x) L ## #x
        F(1)
    ";
    pp!(src, r#"
        L"1"
    "#);

    let src = "
        #define F(x) #x ## _suffix
        F(1)
    ";
    pp!(src, r#"
        "1"_suffix
    "#);

    // stringize des concats dans un va_opt
    let src = "
        #define F(...) #__VA_OPT__(1 ## 2)
        F(_)
    ";
    pp!(src, r#"
        "12"
    "#);

    let src = "
        #define F(...) #__VA_OPT__(__VA_ARGS__ ## __VA_ARGS__)
        F()
        F(1)
    ";
    pp!(src, r#"
        ""
        "11"
    "#);

    let src = "
        #define F(x, ...) #__VA_OPT__(x ## x)
        F(, _)
        F(1, _)
    ";
    pp!(src, r#"
        ""
        "11"
    "#);
}

/// teste qu'on a le bon résultat à la fois dans la sortie du préprocesseur et
/// dans les chaînes de caractères créées par stringification
macro_rules! pp_stringized {
    ($prelude:expr, $tokens:expr, $expected:expr) => {{
        pp!(&format!("{}\n{}", $prelude, $tokens), $expected);

        let src = format!("
            {}
            #define STR(x) #x
            #define STRX(x) STR(x)
            STRX({})
        ", $prelude, $tokens.trim());
        // c'est pas une façon correcte d'escape mais ça suffit pour les tests ici
        let expected = $expected.trim().chars().flat_map(|c| c.escape_default()).collect::<String>();
        pp!(&src, format!("\"{}\"", &expected));
    }};
}

#[test]
fn spacing() {
    // on teste chaque cas avec/sans espace avant/après
    pp_stringized!(
        "#define A 1",
        "+ A +A A+ A +",
        "+ 1 +1 1+ 1 +"
    );

    pp_stringized!(
        "
            #define A + B +B B+ B +
            #define B 1
        ",
        "; A ;A A; A ;",
        "; + 1 +1 1+ 1 + ;+ 1 +1 1+ 1 + + 1 +1 1+ 1 +; + 1 +1 1+ 1 + ;"
    );

    pp_stringized!(
        "#define F(x) x",
        "+ F(1) +F(1) F(1)+ F(1) +",
        "+ 1 +1 1+ 1 +"
    );

    pp_stringized!(
        "
            #define F(x) + G(x) +G(x) G(x)+ G(x) +
            #define G(x) x
        ",
        "; F(1) ;F(1) F(1); F(1) ;",
        "; + 1 +1 1+ 1 + ;+ 1 +1 1+ 1 + + 1 +1 1+ 1 +; + 1 +1 1+ 1 + ;"
    );

    // avec opérateur #
    pp_stringized!(
        "#define F(x) #x",
        "+ F(1) +F(1) F(1)+ F(1) +",
        r#"+ "1" +"1" "1"+ "1" +"#
    );

    pp_stringized!(
        "#define F(x) + #x +#x #x+ #x +",
        "; F(1) ;F(1) F(1); F(1) ;",
        r#"; + "1" +"1" "1"+ "1" + ;+ "1" +"1" "1"+ "1" + + "1" +"1" "1"+ "1" +; + "1" +"1" "1"+ "1" + ;"#
    );

    // avec opérateur ##
    pp_stringized!(
        "#define F(x) x ## 2",
        "+ F(1) +F(1) F(1)+ F(1) +",
        "+ 12 +12 12+ 12 +"
    );

    pp_stringized!(
        "#define F(x) 2 ## x",
        "+ F(1) +F(1) F(1)+ F(1) +",
        "+ 21 +21 21+ 21 +"
    );

    pp_stringized!(
        "#define F(x) + x ## 2 +x ## 2 x ## 2+ x ## 2 +",
        "; F(1) ;F(1) F(1); F(1) ;",
        "; + 12 +12 12+ 12 + ;+ 12 +12 12+ 12 + + 12 +12 12+ 12 +; + 12 +12 12+ 12 + ;"
    );

    pp_stringized!(
        "#define F(x) + 2 ## x +2 ## x 2 ## x+ 2 ## x +",
        "; F(1) ;F(1) F(1); F(1) ;",
        "; + 21 +21 21+ 21 + ;+ 21 +21 21+ 21 + + 21 +21 21+ 21 +; + 21 +21 21+ 21 + ;"
    );

    // 3 concats consécutifs, pas d'espace avant le premier
    pp_stringized!(
        "#define A +1## 2## 3",
        ";A;",
        ";+123;"
    );
    // espace avant le premier
    pp_stringized!(
        "#define A + 1##2##3",
        ";A;",
        ";+ 123;"
    );

    // espaces dans les arguments
    pp_stringized!(
        "#define F(x) x",
        "+F( 1 * 2 ) F( 1* 2 ) F( 1 *2 ) F( 1*2 )+",
        "+1 * 2 1* 2 1 *2 1*2+"
    );

    pp_stringized!(
        "#define F(x) x ## 3",
        "+F( 1 * 2 ) F( 1* 2 ) F( 1 *2 ) F( 1*2 )+",
        "+1 * 23 1* 23 1 *23 1*23+"
    );

    pp_stringized!(
        "#define F(x) 3 ## x",
        "+F( 1 * 2 ) F( 1* 2 ) F( 1 *2 ) F( 1*2 )+",
        "+31 * 2 31* 2 31 *2 31*2+"
    );

    // avec plusieurs paramètres
    pp_stringized!(
        "#define F(x, y) x+y",
        "+F( 1 * 2, 3 * 4 ) F( 1* 2, 3* 4 ) F( 1 *2, 3 *4 ) F( 1*2, 3*4 )+",
        "+1 * 2+3 * 4 1* 2+3* 4 1 *2+3 *4 1*2+3*4+"
    );

    // les espaces avant/après le contenu de va_opt sont ignorés, car il me semble que
    // le contenu de va_opt se comporte comme une replacement list, et ils sont
    // ignorés dans une replacement list mais peut-être que c'est faux
    // (GCC les ignore, Clang les met, MSVC les met au début mais pas à la fin 🙃)
    pp_stringized!(
        "#define F(...) ;__VA_OPT__(1+2); ;__VA_OPT__( 1+2); ;__VA_OPT__(1+2 ); ;__VA_OPT__( 1+2 );",
        "+F(_)+",
        "+;1+2; ;1+2; ;1+2; ;1+2;+"
    );

    // espace entre 2 va_opt
    pp_stringized!(
        "#define F(...) __VA_OPT__(1) __VA_OPT__(2)",
        "+F(_)+",
        "+1 2+"
    );
    // sans espace, dans un stringize on ne met pas d'espace car il n'y en a pas
    // mais dans la sortie formatée on en met parce que sinon ça formerait un
    // token unique `12` au lieu de `1 2` et on a dit qu'on voulait pas former
    // des tokens qui existaient pas à la base
    let src = "
        #define F(...) __VA_OPT__(1)__VA_OPT__(2)
        #define STR(x) #x
        #define STRX(x) STR(x)
        +F(_)+
        STRX(+F(_)+)
    ";
    pp!(src, r#"
        +1 2+
        "+12+"
    "#);

    // chaque instance du paramètre a le bon spacing
    pp_stringized!(
        "#define F(x) x + x+x",
        "+F(1)+",
        "+1 + 1+1+"
    );

    // si un token génère du vide, l'espace avant/après doit bien être respecté
    pp_stringized!(
        "#define EMPTY",
        ";EMPTY; ; EMPTY; ;EMPTY ; ; EMPTY ;",
        ";; ; ; ; ; ; ;"
    );

    pp_stringized!(
        "
            #define EMPTY
            #define A ;EMPTY; ; EMPTY; ;EMPTY ; ; EMPTY ;
        ",
        "+A+",
        "+;; ; ; ; ; ; ;+"
    );

    pp_stringized!(
        "
            #define CC(a, b) a ## b
            #define EMPTY
        ",
        ";CC(EM,PTY); ; CC(EM,PTY); ;CC(EM,PTY) ; ; CC(EM,PTY) ;",
        ";; ; ; ; ; ; ;"
    );

    pp_stringized!(
        "
            #define CC(a, b) a ## b
            #define F(a, b) ;CC(a,b); ; CC(a,b); ;CC(a,b) ; ; CC(a,b) ;
            #define EMPTY
        ",
        "+F(EM, PTY)+",
        "+;; ; ; ; ; ; ;+"
    );

    // avec une fn-like macro empty
    pp_stringized!(
        "#define EMPTY()",
        ";EMPTY(); ; EMPTY(); ;EMPTY() ; ; EMPTY() ;",
        ";; ; ; ; ; ; ;"
    );

    // avec un arg empty
    pp_stringized!(
        "
            #define F(a) ;a; ; a; ;a ; ; a ;
            #define EMPTY
        ",
        "+F(EMPTY)+",
        "+;; ; ; ; ; ; ;+"
    );

    // avec un va_opt empty
    pp_stringized!(
        "
            #define F(...) ;__VA_OPT__(); ; __VA_OPT__(); ;__VA_OPT__() ; ; __VA_OPT__() ;
        ",
        "+F(_)+",
        "+;; ; ; ; ; ; ;+"
    );

    pp_stringized!(
        "
            #define CC(a, b) a ## b
            #define F(a) ;a; ; a; ;a ; ; a ;
            #define EMPTY
        ",
        "+F(CC(EM, PTY))+",
        "+;; ; ; ; ; ; ;+"
    );

    pp_stringized!(
        "
            #define F(a) a
            #define EMPTY
        ",
        "+F(;EMPTY; ; EMPTY; ;EMPTY ; ; EMPTY ;)+",
        "+;; ; ; ; ; ; ;+"
    );

    pp_stringized!(
        "
            #define CC(a, b) a ## b
            #define F(a) a
            #define EMPTY
        ",
        "+F(;CC(EM, PTY); ; CC(EM, PTY); ;CC(EM, PTY) ; ; CC(EM, PTY) ;)+",
        "+;; ; ; ; ; ; ;+"
    );

    // avec un concat empty
    pp_stringized!(
        "
            #define F(a) ;a##a; ; a##a; ;a##a ; ; a##a ;
            #define FX(x) F(x)
            #define EMPTY
        ",
        "+F()+ +FX(EMPTY)+",
        "+;; ; ; ; ; ; ;+ +;; ; ; ; ; ; ;+"
    );
    // dans un va_opt
    pp_stringized!(
        "
            #define F(a, ...) ;__VA_OPT__(a##a); ; __VA_OPT__(a##a); ;__VA_OPT__(a##a) ; ; __VA_OPT__(a##a) ;
            #define FX(x) F(x, _)
            #define EMPTY
        ",
        "+F(_)+ +FX(EMPTY)+",
        "+;; ; ; ; ; ; ;+ +;; ; ; ; ; ; ;+"
    );

    // on teste les cas où le premier/dernier token est à l'intérieur/extérieur
    // de F, pour tester la propagation des espaces au-delà de la frontière de
    // la macro
    // premier token à l'intérieur (+), sans espace
    pp_stringized!(
        "
            #define F(a) +a##a
            #define FX(x) F(x)
            #define EMPTY
        ",
        ";F(); ; F(); ;F() ; ; F() ; ;FX(EMPTY); ; FX(EMPTY); ;FX(EMPTY) ; ; FX(EMPTY) ;",
        ";+; ; +; ;+ ; ; + ; ;+; ; +; ;+ ; ; + ;"
    );
    // premier token à l'intérieur, avec espace
    pp_stringized!(
        "
            #define F(a) + a##a
            #define FX(x) F(x)
            #define EMPTY
        ",
        ";F(); ; F(); ;F() ; ; F() ; ;FX(EMPTY); ; FX(EMPTY); ;FX(EMPTY) ; ; FX(EMPTY) ;",
        ";+ ; ; + ; ;+ ; ; + ; ;+ ; ; + ; ;+ ; ; + ;"
    );
    // dernier token à l'intérieur, sans espace
    pp_stringized!(
        "
            #define F(a) a##a+
            #define FX(x) F(x)
            #define EMPTY
        ",
        ";F(); ; F(); ;F() ; ; F() ; ;FX(EMPTY); ; FX(EMPTY); ;FX(EMPTY) ; ; FX(EMPTY) ;",
        ";+; ; +; ;+ ; ; + ; ;+; ; +; ;+ ; ; + ;"
    );
    // dernier token à l'intérieur, avec espace
    pp_stringized!(
        "
            #define F(a) a##a +
            #define FX(x) F(x)
            #define EMPTY
        ",
        ";F(); ; F(); ;F() ; ; F() ; ;FX(EMPTY); ; FX(EMPTY); ;FX(EMPTY) ; ; FX(EMPTY) ;",
        "; +; ; +; ; + ; ; + ; ; +; ; +; ; + ; ; + ;"
    );

    // pareil mais dans une obj-like macro
    pp_stringized!(
        "
            #define A +EM##PTY
            #define EMPTY
        ",
        ";A; ; A; ;A ; ; A ;",
        ";+; ; +; ;+ ; ; + ;"
    );
    pp_stringized!(
        "
            #define A + EM##PTY
            #define EMPTY
        ",
        ";A; ; A; ;A ; ; A ;",
        ";+ ; ; + ; ;+ ; ; + ;"
    );
    pp_stringized!(
        "
            #define A EM##PTY+
            #define EMPTY
        ",
        ";A; ; A; ;A ; ; A ;",
        ";+; ; +; ;+ ; ; + ;"
    );
    pp_stringized!(
        "
            #define A EM##PTY +
            #define EMPTY
        ",
        ";A; ; A; ;A ; ; A ;",
        "; +; ; +; ; + ; ; + ;"
    );

    pp_stringized!(
        "#define F(a) a ## a",
        ";F(); ; F(); ; F(); ; F() ;",
        ";; ; ; ; ; ; ;"
    );

    pp_stringized!(
        "#define F(a) a",
        ";F(); ; F(); ;F() ; ; F() ;",
        ";; ; ; ; ; ; ;"
    );

    // empty avant l'opérateur #
    pp_stringized!(
        "
            #define F(x) ;EMPTY#x; ; EMPTY#x; ;EMPTY#x ; ; EMPTY#x ;
            #define EMPTY
        ",
        "+F(1)+",
        r#"+;"1"; ; "1"; ;"1" ; ; "1" ;+"#
    );

    // les newlines ne sont pas conservés dans les macros
    pp_stringized!(
        r"
            #define A int main() \
                { \
                    return 0; \
                }
        ",
        "A",
        "int main() { return 0; }"
    );
    // ni dans les arguments (il n'y a pas d'espace avant le '3', '+' et '2' mais
    // vu qu'il y a un newline avant on met un espace quand même)
    pp_stringized!(
        r"#define F(a) a + \
3
        ",
        "
F(1
+
2)
        ",
        "1 + 2 + 3"
    );

    // pas d'espace avant le 2
    pp_stringized!(
        r"#define A 1 +\
2
        ",
        "A",
        "1 +2"
    );

    // ici il y a un espace sur la ligne précédente (avant la line continuation)
    // donc on met un espace
    pp_stringized!(
        r"#define A 1 + \
2
        ",
        "A",
        "1 + 2"
    );

    pp_stringized!(
        r"
            #define F(a) \
                G(a + 4, 5 * 6) + \
                    7
            #define G(x, y) x \
                + \
                y
            #define H(a) 8 + a - 9
        ",
        "
            F(H(1
            +
            2) - 3)
        ",
        "8 + 1 + 2 - 9 - 3 + 4 + 5 * 6 + 7"
    );

    // les line continuations ne comptent pas comme des newlines
    pp_stringized!(
        "",
        r"
            abc \
            def \
            blabla
        ",
        "abc def blabla"
    );
}

#[test]
fn formatted_pp_output() {
    add_file("src/formatted.hpp", "
    #define A 24
 A
  void foo()
  {
  }
    ");

    // le contenu des fichiers inclus respecte l'indentation originale, quelque soit
    // l'endroit où ça a été inclus
    let src = r#"
        a
        #include "formatted.hpp"
        b
    "#;
    pp!(src, "
        a
 24
  void foo()
  {
  }
        b
    ");

    // on veut pas que 1 token se forme à partir de 2 tokens (si jamais on veut
    // preprocess à nouveau le texte préprocessé) donc il faut rajouter des espaces
    // par-ci par-là même si ils ne sont pas dans le code original, pour éviter
    // de former des tokens qui n'y étaient pas

    // //
    let src = "
        #define A /
        A/ /A pas un commentaire // commentaire
    ";
    pp!(src, "/ / / / pas un commentaire");

    // /*
    let src = "
        #define A /
        #define B *
        A* /B pas un commentaire /* commentaire */
    ";
    pp!(src, "/ * / * pas un commentaire");

    // ##
    let src = "
        #define A #
        A# #A ##
    ";
    pp!(src, "# # # # ##");

    // %:
    let src = "
        #define A %
        #define B :
        A: %B %:
    ";
    pp!(src, "% : % : %:");

    // %:%:
    let src = "
        #define A %
        #define B :
        A:A: %B%B A:%: %:A: %B%: %:%B %:%:
    ";
    pp!(src, "% :% : % :% : % :%: %:% : % :%: %:% : %:%:");

    // [:
    let src = "
        #define A [
        #define B :
        A: [B [:
    ";
    pp!(src, "[ : [ : [:");

    // :]
    let src = "
        #define A :
        #define B ]
        A] :B :]
    ";
    pp!(src, ": ] : ] :]");

    // <%
    let src = "
        #define A <
        #define B %
        A% <B <%
    ";
    pp!(src, "< % < % <%");

    // %>
    let src = "
        #define A %
        #define B >
        A> %B %>
    ";
    pp!(src, "% > % > %>");

    // <:
    let src = "
        #define A <
        #define B :
        A: <B <:
    ";
    pp!(src, "< : < : <:");

    // :>
    let src = "
        #define A :
        #define B >
        A> :B :>
    ";
    pp!(src, ": > : > :>");

    // ...
    // todo: on gènère trop d'espaces, on pourrait faire par ex ". .. . . . .. . ..."
    // mais c'est un peu chiant à faire pour rien
    let src = "
        #define A .
        A.. .A. ..A ...
    ";
    pp!(src, ". . . . . . . . . ...");

    // ::
    let src = "
        #define A :
        A: :A ::
    ";
    pp!(src, ": : : : ::");

    // .*
    let src = "
        #define A .
        #define B *
        A* .B .*
    ";
    pp!(src, ". * . * .*");

    // ->
    let src = "
        #define A -
        #define B >
        A> -B ->
    ";
    pp!(src, "- > - > ->");

    // ->*
    let src = "
        #define A -
        #define B >
        #define C *
        A>* -B* ->C ->*
    ";
    pp!(src, "- >* - >* -> * ->*");

    // ^^
    let src = "
        #define A ^
        A^ ^A ^^
    ";
    pp!(src, "^ ^ ^ ^ ^^");

    // +=
    let src = "
        #define A +
        #define B =
        A= +B +=
    ";
    pp!(src, "+ = + = +=");

    // -=
    let src = "
        #define A -
        #define B =
        A= -B -=
    ";
    pp!(src, "- = - = -=");

    // *=
    let src = "
        #define A *
        #define B =
        A= *B *=
    ";
    pp!(src, "* = * = *=");

    // /=
    let src = "
        #define A /
        #define B =
        A= /B /=
    ";
    pp!(src, "/ = / = /=");

    // %=
    let src = "
        #define A %
        #define B =
        A= %B %=
    ";
    pp!(src, "% = % = %=");

    // ^=
    let src = "
        #define A ^
        #define B =
        A= ^B ^=
    ";
    pp!(src, "^ = ^ = ^=");

    // &=
    let src = "
        #define A &
        #define B =
        A= &B &=
    ";
    pp!(src, "& = & = &=");

    // |=
    let src = "
        #define A |
        #define B =
        A= |B |=
    ";
    pp!(src, "| = | = |=");

    // ==
    let src = "
        #define A =
        A= =A ==
    ";
    pp!(src, "= = = = ==");

    // !=
    let src = "
        #define A !
        #define B =
        A= !B !=
    ";
    pp!(src, "! = ! = !=");

    // <=
    let src = "
        #define A <
        #define B =
        A= <B <=
    ";
    pp!(src, "< = < = <=");

    // >=
    let src = "
        #define A >
        #define B =
        A= >B >=
    ";
    pp!(src, "> = > = >=");

    // <=>
    let src = "
        #define A <
        #define B =
        #define C >
        A=> <B> <=C <=>
    ";
    pp!(src, "< => < => <= > <=>");

    // &&
    let src = "
        #define A &
        A& &A &&
    ";
    pp!(src, "& & & & &&");

    // ||
    let src = "
        #define A |
        A| |A ||
    ";
    pp!(src, "| | | | ||");

    // <<
    let src = "
        #define A <
        A< <A <<
    ";
    pp!(src, "< < < < <<");

    // >>
    let src = "
        #define A >
        A> >A >>
    ";
    pp!(src, "> > > > >>");

    // <<=
    let src = "
        #define A <
        #define B =
        A<= <A= <<B <<=
    ";
    pp!(src, "< <= < < = << = <<=");

    // >>=
    let src = "
        #define A >
        #define B =
        A>= >A= >>B >>=
    ";
    pp!(src, "> >= > > = >> = >>=");

    // ++
    let src = "
        #define A +
        A+ +A ++
    ";
    pp!(src, "+ + + + ++");

    // --
    let src = "
        #define A -
        A- -A --
    ";
    pp!(src, "- - - - --");

    // pas une wide string
    let src = r#"
        #define A L
        A"salut"
    "#;
    pp!(src, r#"
        L "salut"
    "#);
}

macro_rules! eval {
    ($src:expr, $expected:expr) => {{
        let mut shub = SourceHub::new();
        let source_id = shub.add_source("".into(), $src.to_owned()).id();
        let mut diags = Diags::new();

        let mut pp = Preprocessor::new(PpOptions::default(), &mut shub, &mut diags, &TestFileLoader);
        let tokens = pp.preprocess(source_id);

        let ops = ExprParser::new(&tokens, &mut shub, &mut diags).parse().unwrap();
        assert!(diags.diags().is_empty());
        assert_eq!(Interpreter::new().eval(&ops), $expected);
    }};
}

#[test]
fn expr_eval() {
    let src = "
        #define FOO 1
        #if FOO == 1
        A
        #endif
    ";
    pp!(src, "A");

    // identifier non défini vaut 0
    let src = "
        #if FOO == 0
        A
        #endif
    ";
    pp!(src, "A");

    // on a le droit d'utiliser des kw
    let src = "
        #if struct == 0
        A
        #endif
    ";
    pp!(src, "A");

    // appel de macro
    let src = "
        #define F(x) x - 1
        #if F(1) == 0
        A
        #else
        B
        #endif
    ";
    pp!(src, "A");

    // ces tests ne sont pas exhaustifs mais on s'en fout c'est juste un interpréteur
    // temporaire en attendant d'en avoir un vrai
    eval!("+1", 1);
    eval!("-1", -1);
    eval!("1 + 2", 3);
    eval!("4 / 2", 2);
    eval!("3 % 2", 1);
    eval!("~0", -1);
    eval!("0b101 & 0b100", 0b100);
    eval!("0b101 | 0b100", 0b101);
    eval!("0b101 ^ 0b100", 0b001);
    eval!("1 << 32", 1 << 32);
    eval!("8 >> 2", 2);
    eval!("!0", 1);
    eval!("!1", 0);
    eval!("false && true", 0);
    eval!("false || true", 1);
    eval!("1 == 1", 1);
    eval!("1 == 2", 0);
    eval!("1 != 1", 0);
    eval!("1 != 2", 1);
    eval!("1 < 2", 1);
    eval!("2 < 1", 0);
    eval!("1 <= 2", 1);
    eval!("2 <= 1", 0);
    eval!("1 > 2", 0);
    eval!("2 > 1", 1);
    eval!("1 >= 2", 0);
    eval!("2 >= 1", 1);
    eval!("1 == 2 ? 42 : 69", 69);
    eval!("true ? false ? 1 : 2 : 3", 2);

    eval!("1 + 2 * 3", 7);
    eval!("(1 + 2) * 3", 9);

    // expr invalide
    // float
    diags!("#if 3.5 \n #endif", [InvalidExpr(vec![ExprError::Float(span(4..7))], kw::If)]);
    diags!("#if 3.5f \n #endif", [InvalidExpr(vec![ExprError::Float(span(4..8))], kw::If)]);

    // str
    diags!("#if \"bla\" \n #endif", [InvalidExpr(vec![ExprError::Str(span(4..9))], kw::If)]);
    diags!("#if u8\"bla\" \n #endif", [InvalidExpr(vec![ExprError::Str(span(4..11))], kw::If)]);
    diags!("#if u\"bla\" \n #endif", [InvalidExpr(vec![ExprError::Str(span(4..10))], kw::If)]);
    diags!("#if U\"bla\" \n #endif", [InvalidExpr(vec![ExprError::Str(span(4..10))], kw::If)]);
    diags!("#if R\"(bla)\" \n #endif", [InvalidExpr(vec![ExprError::Str(span(4..12))], kw::If)]);

    // user-defined suffix
    diags!("#if 3_a \n #endif", [InvalidExpr(vec![ExprError::UdSuffix(span(4..7))], kw::If)]);
    diags!("#if 3.5_a \n #endif", [InvalidExpr(vec![ExprError::Float(span(4..9))], kw::If)]);
    diags!("#if 'a'_a \n #endif", [InvalidExpr(vec![ExprError::UdSuffix(span(4..9))], kw::If)]);
    diags!("#if \"bla\"_a \n #endif", [InvalidExpr(vec![ExprError::Str(span(4..11))], kw::If)]);

    // todo: peut-être qu'on veut qu'une seule erreur ici ?
    diags!("#if ( \n #endif", [InvalidExpr(vec![
        ExprError::ExpectedExpr(span(4..5)),
        ExprError::UnmatchedParen { span: span(4..5), is_left: true },
    ], kw::If)]);
    diags!("#if (1 \n #endif", [InvalidExpr(vec![ExprError::UnmatchedParen { span: span(4..5), is_left: true }], kw::If)]);
    diags!("#if ) \n #endif", [InvalidExpr(vec![ExprError::UnmatchedParen { span: span(4..5), is_left: false }], kw::If)]);
    diags!("#if 1) \n #endif", [InvalidExpr(vec![ExprError::UnmatchedParen { span: span(5..6), is_left: false }], kw::If)]);
    diags!("#if () \n #endif", [InvalidExpr(vec![ExprError::EmptyParens(span(4..6))], kw::If)]);

    diags!("#if + \n #endif", [InvalidExpr(vec![ExprError::ExpectedExpr(span(4..5))], kw::If)]);
    diags!("#if 1 + \n #endif", [InvalidExpr(vec![ExprError::ExpectedExpr(span(6..7))], kw::If)]);

    diags!("#if 0 ? \n #endif", [InvalidExpr(vec![
        ExprError::ExpectedExpr(span(6..7)),
        ExprError::QuestionWithoutColon(span(6..7)),
    ], kw::If)]);
    diags!("#if 0 ? 1 : \n #endif", [InvalidExpr(vec![ExprError::ExpectedExpr(span(10..11))], kw::If)]);
    diags!("#if 0 ? 1 \n #endif", [InvalidExpr(vec![ExprError::QuestionWithoutColon(span(6..7))], kw::If)]);
    diags!("#if 0 ? : 1 \n #endif", [InvalidExpr(vec![ExprError::ExpectedExpr(span(6..7))], kw::If)]);

    diags!("#if 1 2 3 \n #endif", [InvalidExpr(vec![ExprError::UnexpectedToken(span(6..7))], kw::If)]);

    // opérateurs invalides
    diags!("#if a = 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..7), BinOpKind::Assign)], kw::If)]);
    diags!("#if a += 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a *= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a /= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a %= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a &= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a |= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a ^= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..8), BinOpKind::Assign)], kw::If)]);
    diags!("#if a <<= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..9), BinOpKind::Assign)], kw::If)]);
    diags!("#if a >>= 0 \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..9), BinOpKind::Assign)], kw::If)]);
    diags!("#if a++ \n #endif", [InvalidExpr(vec![ExprError::UnexpectedToken(span(5..7))], kw::If)]);
    diags!("#if a-- \n #endif", [InvalidExpr(vec![ExprError::UnexpectedToken(span(5..7))], kw::If)]);
    diags!("#if ++a \n #endif", [InvalidExpr(vec![ExprError::InvalidUnOp(span(4..6), UnOpKind::Other)], kw::If)]);
    diags!("#if --a \n #endif", [InvalidExpr(vec![ExprError::InvalidUnOp(span(4..6), UnOpKind::Other)], kw::If)]);
    diags!("#if a <=> b \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(6..9), BinOpKind::Other)], kw::If)]);
    diags!("#if a[3] \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..6), BinOpKind::Subscript)], kw::If)]);
    diags!("#if *a \n #endif", [InvalidExpr(vec![ExprError::InvalidUnOp(span(4..5), UnOpKind::Deref)], kw::If)]);
    diags!("#if &a \n #endif", [InvalidExpr(vec![ExprError::InvalidUnOp(span(4..5), UnOpKind::AddrOf)], kw::If)]);
    diags!("#if a->b \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..7), BinOpKind::Other)], kw::If)]);
    diags!("#if a.b \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..6), BinOpKind::Other)], kw::If)]);
    diags!("#if a->*b \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..8), BinOpKind::Other)], kw::If)]);
    diags!("#if a.*b \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..7), BinOpKind::Other)], kw::If)]);
    diags!("#if a(b) \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..6), BinOpKind::FnCall)], kw::If)]);
    // c'est pas clair si la virgule est censée être autorisée, en attendant on l'interdit
    diags!("#if a, b \n #endif", [InvalidExpr(vec![ExprError::InvalidBinOp(span(5..6), BinOpKind::Comma)], kw::If)]);
}

#[test]
fn other_tests() {
    // tests en vrac extraits du standard, il faut bien vérifier qu'ils passent,
    // c'est quand même la moindre des choses (:
    let src = "
        #define LPAREN() (
        #define G(Q) 42
        #define F(R, X, ...)  __VA_OPT__(G R X) )
        int x = F(LPAREN(), 0, <:-);
    ";
    pp!(src, "int x = 42;");

    let src = r#"
        #define debug(...) fprintf(stderr, __VA_ARGS__)
        #define showlist(...) puts(#__VA_ARGS__)
        #define report(test, ...) ((test) ? puts(#test) : printf(__VA_ARGS__))
        debug("Flag");
        debug("X = %d\n", x);
        showlist(The first, second, and third items.);
        report(x>y, "x is %d but y is %d", x, y);
    "#;
    pp!(src, r#"
        fprintf(stderr, "Flag");
        fprintf(stderr, "X = %d\n", x);
        puts("The first, second, and third items.");
        ((x>y) ? puts("x>y") : printf("x is %d but y is %d", x, y));
    "#);

    let src = "
        #define F(...)           f(0 __VA_OPT__(,) __VA_ARGS__)
        #define G(X, ...)        f(0, X __VA_OPT__(,) __VA_ARGS__)
        #define SDEF(sname, ...) S sname __VA_OPT__(= { __VA_ARGS__ })
        #define EMP
        F(a, b, c)
        F()
        F(EMP)
        G(a, b, c)
        G(a, )
        G(a)
        SDEF(foo);
        SDEF(bar, 1, 2);
    ";
    // dans le standard il y a pas d'espace avant les ',' et ')' mais il en faut,
    // le standard se trompe 🤡
    // non je rigole mais à moins qu'il y ait une règle spéciale pour __VA_OPT__,
    // il y a aucune raison qu'il n'y ait pas d'espaces, donc on en met (les
    // autres compilateurs en mettent aussi)
    pp!(src, "
        f(0 , a, b, c)
        f(0 )
        f(0 )
        f(0, a , b, c)
        f(0, a )
        f(0, a )
        S foo;
        S bar = { 1, 2 };
    ");

    let src = "
        #define H2(X, Y, ...) __VA_OPT__(X ## Y,) __VA_ARGS__
        #define H3(X, ...) #__VA_OPT__(X##X X##X)
        #define H4(X, ...) __VA_OPT__(a X ## X) ## b
        #define H5A(...) __VA_OPT__()/**/__VA_OPT__()
        #define H5B(X) a ## X ## b
        #define H5C(X) H5B(X)

        H2(a, b, c, d)
        H3(, 0)
        H4(, 1)
        H5C(H5A())
    ";
    pp!(src, r#"
        ab, c, d
        ""
        a b
        ab
    "#);

    let src = "
        #define x       3
        #define f(a)    f(x * (a))
        #undef  x
        #define x       2
        #define g       f
        #define z       z[0]
        #define h       g(~
        #define m(a)    a(w)
        #define w       0,1
        #define t(a)    a
        #define p()     int
        #define q(x)    x
        #define r(x,y)  x ## y
        #define str(x)  # x

        f(y+1) + f(f(z)) % t(t(g)(0) + t)(1);
        g(x+(3,4)-w) | h 5) & m
            (f)^m(m);
        p() i[q()] = { q(1), r(2,3), r(4,), r(,5), r(,) };
        char c[2][6] = { str(hello), str() };
    ";
    pp!(src, r#"
        f(2 * (y+1)) + f(2 * (f(2 * (z[0])))) % f(2 * (0)) + t(1);
        f(2 * (2+(3,4)-0,1)) | f(2 * (~ 5)) & f(2 * (0,1))^m(0,1);
        int i[] = { 1, 23, 4, 5, };
        char c[2][6] = { "hello", "" };
    "#);

    // test extrait de https://marc.info/?l=boost&m=118835769257658
    let src = "
        #define INTERNAL_CAT(a, b) INTERNAL_PRIMITIVE_CAT(a, b)
        #define INTERNAL_PRIMITIVE_CAT(a, b) a ## b
        #define EMPTY()
        #define CAT_1(a, b) PRIMITIVE_CAT_1(a, b)
        #define CAT_1_ID() CAT_1
        #define PRIMITIVE_CAT_1(a, b) a ## b
        #define CAT_2(a, b) PRIMITIVE_CAT_2(a, b)
        #define CAT_2_ID() CAT_2
        #define PRIMITIVE_CAT_2(a, b) a ## b
        #define CAT_3(a, b) PRIMITIVE_CAT_3(a, b)
        #define CAT_3_ID() CAT_3
        #define PRIMITIVE_CAT_3(a, b) a ## b
        #define CAT_4(a, b) PRIMITIVE_CAT_4(a, b)
        #define CAT_4_ID() CAT_4
        #define PRIMITIVE_CAT_4(a, b) a ## b
        #define CAT TRY_1()
        #define TRY_1() INTERNAL_CAT(TRY_1_, CAT_1(0, 0))()
        #define TRY_2() INTERNAL_CAT(TRY_2_, CAT_2(0, 0))()
        #define TRY_3() INTERNAL_CAT(TRY_3_, CAT_3(0, 0))()
        #define TRY_4() INTERNAL_CAT(TRY_4_, CAT_4(0, 0))()
        #define TRY_1_00 CAT_1_ID
        #define TRY_2_00 CAT_2_ID
        #define TRY_3_00 CAT_3_ID
        #define TRY_4_00 CAT_4_ID
        #define TRY_1_CAT_1(a, b) TRY_2
        #define TRY_2_CAT_2(a, b) TRY_3
        #define TRY_3_CAT_3(a, b) TRY_4
        #define TRY_4_CAT_4(a, b) error: ran out of CAT depth! EMPTY
        CAT(C, AT(C, AT(1, 2)))
    ";
    pp!(src, "12");
}
