#![feature(bstr)]
#![feature(custom_inner_attributes)]
#![rustfmt::skip]

use exx::lex::*;
use std::bstr::ByteString;
use std::num::NonZeroU32;
use TokenKind::*;

/// assert que les tokens de la source sont les tokens attendus et qu'il y a pas d'erreur
macro_rules! tokens {
    ($src:expr, [$($token:expr),* $(,)?]) => {
        tokens_and_errors!($src, [$($token,)*], []);
    };
}

/// assert qu'il y a les erreurs attendues
macro_rules! errors {
    ($src:expr, [$($error:expr),* $(,)?]) => {
        {
            let mut lexer = Lexer::new($src);
            while lexer.lex().0 != TokenKind::Eof {}

            let expected_errors = [$($error,)*];
            assert_eq!(lexer.errors(), &expected_errors);
        }
    };
}

/// assert qu'il y a les tokens et les erreurs attendues
macro_rules! tokens_and_errors {
    ($src:expr, [$($token:expr),* $(,)?], [$($error:expr),* $(,)?]) => {
        {
            let mut lexer = Lexer::new($src);
            let mut tokens = Vec::new();
            while let (kind, span) = lexer.lex() && kind != TokenKind::Eof {
                tokens.push((kind, span));
            }

            let expected_errors = [$($error,)*];
            assert_eq!(lexer.errors(), &expected_errors);

            let expected_tokens = [$($token,)*];
            assert_eq!(tokens, expected_tokens);
        }
    };
}

/// assert qu'il n'y a qu'un seul token dont le texte (lexème) est celui attendu
macro_rules! lexeme {
    ($src:expr, $expected:expr) => {
        {
            let mut lexer = Lexer::new($src);
            let mut tokens = Vec::new();
            while let (kind, span) = lexer.lex() && kind != TokenKind::Eof {
                tokens.push((kind, span));
            }
            assert_eq!(tokens.len(), 1);

            let (kind, span) = &tokens[0];
            let actual = extract_lexeme(kind, &$src[span.start as usize..span.end as usize]);
            assert_eq!(actual, $expected);
        }
    };
}

fn to_utf8(s: &str) -> ByteString {
    let mut vec = s.as_bytes().to_vec();
    vec.push(0);
    ByteString(vec)
}

fn to_utf16(s: &str) -> ByteString {
    let mut vec: Vec<_> = s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    vec.push(0);
    ByteString(vec)
}

fn to_utf32(s: &str) -> ByteString {
    let mut vec: Vec<_> = s.chars().flat_map(|c| (c as u32).to_le_bytes()).collect();
    vec.push(0);
    ByteString(vec)
}

fn name(s: &str) -> TokenKind {
    Name(exx::name::Name::from(s))
}

#[test]
fn operators() {
    tokens!("#", [(Hash, 0..1)]);
    tokens!("##", [(HashHash, 0..2)]);
    tokens!("{", [(BraceL, 0..1)]);
    tokens!("}", [(BraceR, 0..1)]);
    tokens!("[", [(BracketL, 0..1)]);
    tokens!("]", [(BracketR, 0..1)]);
    tokens!("(", [(ParenL, 0..1)]);
    tokens!(")", [(ParenR, 0..1)]);
    tokens!("[:", [(SpliceL, 0..2)]);
    tokens!(":]", [(SpliceR, 0..2)]);
    tokens!(";", [(Semi, 0..1)]);
    tokens!(":", [(Colon, 0..1)]);
    tokens!("...", [(DotDotDot, 0..3)]);
    tokens!("?", [(Question, 0..1)]);
    tokens!("::", [(ColonColon, 0..2)]);
    tokens!(".", [(Dot, 0..1)]);
    tokens!(".*", [(DotStar, 0..2)]);
    tokens!("->", [(Arrow, 0..2)]);
    tokens!("->*", [(ArrowStar, 0..3)]);
    tokens!("^^", [(CatEars, 0..2)]);
    tokens!("~", [(Tilde, 0..1)]);
    tokens!("!", [(Bang, 0..1)]);
    tokens!("+", [(Plus, 0..1)]);
    tokens!("-", [(Minus, 0..1)]);
    tokens!("*", [(Star, 0..1)]);
    tokens!("/", [(Slash, 0..1)]);
    tokens!("%", [(Percent, 0..1)]);
    tokens!("^", [(Caret, 0..1)]);
    tokens!("&", [(And, 0..1)]);
    tokens!("|", [(Or, 0..1)]);
    tokens!("=", [(Eq, 0..1)]);
    tokens!("+=", [(PlusEq, 0..2)]);
    tokens!("-=", [(MinusEq, 0..2)]);
    tokens!("*=", [(StarEq, 0..2)]);
    tokens!("/=", [(SlashEq, 0..2)]);
    tokens!("%=", [(PercentEq, 0..2)]);
    tokens!("^=", [(CaretEq, 0..2)]);
    tokens!("&=", [(AndEq, 0..2)]);
    tokens!("|=", [(OrEq, 0..2)]);
    tokens!("==", [(EqEq, 0..2)]);
    tokens!("!=", [(Ne, 0..2)]);
    tokens!("<", [(Lt, 0..1)]);
    tokens!(">", [(Gt, 0..1)]);
    tokens!("<=", [(LtEq, 0..2)]);
    tokens!(">=", [(GtEq, 0..2)]);
    tokens!("<=>", [(Spaceship, 0..3)]);
    tokens!("&&", [(AndAnd, 0..2)]);
    tokens!("||", [(OrOr, 0..2)]);
    tokens!("<<", [(LtLt, 0..2)]);
    tokens!(">>", [(GtGt, 0..2)]);
    tokens!("<<=", [(LtLtEq, 0..3)]);
    tokens!(">>=", [(GtGtEq, 0..3)]);
    tokens!("++", [(PlusPlus, 0..2)]);
    tokens!("--", [(MinusMinus, 0..2)]);
    tokens!(",", [(Comma, 0..1)]);

    // `[:` est bien un splice
    tokens!("[:::", [(SpliceL, 0..2), (ColonColon, 2..4)]);
    // mais pas dans ces cas
    tokens!("[::", [(BracketL, 0..1), (ColonColon, 1..3)]);
    tokens!("[::a", [(BracketL, 0..1), (ColonColon, 1..3), (name("a"), 3..4)]);
    tokens!("[::]", [(BracketL, 0..1), (ColonColon, 1..3), (BracketR, 3..4)]);
    tokens!("[:>", [(BracketL, 0..1), (BracketR, 1..3)]);

    // maximal munch
    tokens!("+++++", [(PlusPlus, 0..2), (PlusPlus, 2..4), (Plus, 4..5)]);
    tokens!("-----", [(MinusMinus, 0..2), (MinusMinus, 2..4), (Minus, 4..5)]);
    tokens!("<<<<<", [(LtLt, 0..2), (LtLt, 2..4), (Lt, 4..5)]);
    tokens!(">>>>>", [(GtGt, 0..2), (GtGt, 2..4), (Gt, 4..5)]);
    tokens!("=====", [(EqEq, 0..2), (EqEq, 2..4), (Eq, 4..5)]);
    tokens!("&&&&&", [(AndAnd, 0..2), (AndAnd, 2..4), (And, 4..5)]);
    tokens!("|||||", [(OrOr, 0..2), (OrOr, 2..4), (Or, 4..5)]);
    tokens!("^^^^^", [(CatEars, 0..2), (CatEars, 2..4), (Caret, 4..5)]);
    tokens!(":::::", [(ColonColon, 0..2), (ColonColon, 2..4), (Colon, 4..5)]);
    tokens!("#####", [(HashHash, 0..2), (HashHash, 2..4), (Hash, 4..5)]);

    // with line continuations
    tokens!("#\\\n#", [(HashHash, 0..4)]);
    lexeme!("#\\\n#", "##");
    tokens!("+\\\n=", [(PlusEq, 0..4)]);
    lexeme!("+\\\n=", "+=");
    tokens!("&\\\n&", [(AndAnd, 0..4)]);
    lexeme!("&\\\n&", "&&");
    tokens!("<\\\n=\\\n>", [(Spaceship, 0..7)]);
    lexeme!("<\\\n=\\\n>", "<=>");
}

#[test]
fn alternative_tokens() {
    tokens!("and", [(AndAnd, 0..3)]);
    tokens!("or", [(OrOr, 0..2)]);
    tokens!("xor", [(Caret, 0..3)]);
    tokens!("not", [(Bang, 0..3)]);
    tokens!("bitand", [(And, 0..6)]);
    tokens!("bitor", [(Or, 0..5)]);
    tokens!("compl", [(Tilde, 0..5)]);
    tokens!("and_eq", [(AndEq, 0..6)]);
    tokens!("or_eq", [(OrEq, 0..5)]);
    tokens!("xor_eq", [(CaretEq, 0..6)]);
    tokens!("not_eq", [(Ne, 0..6)]);
    tokens!("%:", [(Hash, 0..2)]);
    tokens!("%:%:", [(HashHash, 0..4)]);
    tokens!("<%", [(BraceL, 0..2)]);
    tokens!("%>", [(BraceR, 0..2)]);
    tokens!("<:", [(BracketL, 0..2)]);
    tokens!(":>", [(BracketR, 0..2)]);

    // `:]` et `[:` ne peuvent pas être formés à partir d'alternative tokens
    tokens!("::>", [(ColonColon, 0..2), (Gt, 2..3)]);
    // de plus, ceci n'est _pas_ l'alternative token de `[` suivi de `:`
    tokens!("<::", [(Lt, 0..1), (ColonColon, 1..3)]);
    tokens!("<::a", [(Lt, 0..1), (ColonColon, 1..3), (name("a"), 3..4)]);

    // `<:` est bien l'alternative token de `[` dans ces cas
    tokens!("<:a", [(BracketL, 0..2), (name("a"), 2..3)]);
    tokens!("<::>", [(BracketL, 0..2), (BracketR, 2..4)]);
    tokens!("<:::", [(BracketL, 0..2), (ColonColon, 2..4)]);

    // alternative tokens keep source spelling
    lexeme!("and", "and");
    lexeme!("or", "or");
    lexeme!("xor", "xor");
    lexeme!("not", "not");
    lexeme!("bitand", "bitand");
    lexeme!("bitor", "bitor");
    lexeme!("compl", "compl");
    lexeme!("and_eq", "and_eq");
    lexeme!("or_eq", "or_eq");
    lexeme!("xor_eq", "xor_eq");
    lexeme!("not_eq", "not_eq");
    lexeme!("%:", "%:");
    lexeme!("%:%:", "%:%:");
    lexeme!("<%", "<%");
    lexeme!("%>", "%>");
    lexeme!("<:", "<:");
    lexeme!(":>", ":>");

    // with line continuations
    tokens!("a\\\nn\\\nd", [(AndAnd, 0..7)]);
    lexeme!("a\\\nn\\\nd", "and");
    tokens!("<\\\n:", [(BracketL, 0..4)]);
    lexeme!("<\\\n:", "<:");
}

#[test]
fn ident() {
    tokens!("abcdefghijklmnopqrstuvwxyz", [(name("abcdefghijklmnopqrstuvwxyz"), 0..26)]);
    tokens!("ABCDEFGHIJKLMNOPQRSTUVWXYZ", [(name("ABCDEFGHIJKLMNOPQRSTUVWXYZ"), 0..26)]);
    tokens!("a1234567890", [(name("a1234567890"), 0..11)]);
    tokens!("ab_3_cX", [(name("ab_3_cX"), 0..7)]);
    tokens!("_Y_s_", [(name("_Y_s_"), 0..5)]);
    tokens!("éòÿà", [(name("éòÿà"), 0..8)]);
    tokens!("𝐀b𝐀", [(name("𝐀b𝐀"), 0..9)]);

    // `·` == `U+00B7` ne peut pas apparaître au début d'un identifiant
    // (XID_Continue mais pas XID_Start)
    tokens!("·abc", [(Unknown, 0..2), (name("abc"), 2..5)]);
    tokens!("ab·c", [(name("ab·c"), 0..5)]);

    // pas un identifiant
    tokens!("0abc", [(Number, 0..4)]);

    tokens!("ab+c", [(name("ab"), 0..2), (Plus, 2..3), (name("c"), 3..4)]);
    // c'est un `U+00D7`, pas un 'x'
    tokens!("ab×c", [(name("ab"), 0..2), (Unknown, 2..4), (name("c"), 4..5)]);
    // les emojis ne font pas partie des identifiants iiuc ([lex.name])
    // todo: ça serait peut-être mieux de considérer que c'est quand même un
    // identifiant et refuser par la suite au lieu d'avoir un token Unknown débile
    tokens!("ab🤡c", [(name("ab"), 0..2), (Unknown, 2..6), (name("c"), 6..7)]);

    // with UCN
    tokens!(r"a\u00E9c", [(name("aéc"), 0..8)]);
    tokens!(r"\u00E9\u00E0", [(name("éà"), 0..12)]);
    tokens!(r"\U000000E9\u00E0", [(name("éà"), 0..16)]);
    tokens!(r"\N{LATIN SMALL LETTER E WITH ACUTE}\u00E0", [(name("éà"), 0..41)]);

    // with line continuations
    tokens!("a\\\n\\u\\\n00\\\nE9c", [(name("aéc"), 0..14)]);
    lexeme!("a\\\n\\u\\\n00\\\nE9c", "aéc");
}

#[test]
fn comments() {
    // single line comment
    tokens!("//", []);
    tokens!("//a", []);
    tokens!("//\n", []);
    tokens!("//bonjour\n", []);
    tokens!(r"// b/* 🤡 */ n */ \u0061 é // jour + 2 \n", []);
    // followed by something
    tokens!("// bonjour\na", [(name("a"), 11..12)]);
    // different types of newline
    tokens!("// bonjour\r\na", [(name("a"), 12..13)]);
    tokens!("// bonjour\ra", [(name("a"), 11..12)]);
    // preceded by something
    tokens!("a//bonjour\nb", [(name("a"), 0..1), (name("b"), 11..12)]);
    // ending with eof
    tokens!("//bonjour", []);

    // multi line comment
    tokens!("/**/", []);
    tokens!("/* bonjour */", []);
    tokens!("/* * / */", []);
    tokens!(r"/** b🤡n + jour \u0061 2 */", []);
    // they don't nest
    tokens!("/* a /* /* /* b */", []);
    tokens!("/* a /* b */ c */", [(name("c"), 13..14), (Star, 15..16), (Slash, 16..17)]);
    // with single line comment inside
    tokens!("/* ab // cd */", []);
    // with something before/after
    tokens!("a/* bonjour */b", [(name("a"), 0..1), (name("b"), 14..15)]);

    // unterminated
    errors!("/*", [LexError::Unterminated(UnterminatedKind::MultilineComment, 0)]);
    errors!(" /* a", [LexError::Unterminated(UnterminatedKind::MultilineComment, 1)]);
    errors!(" /* a *", [LexError::Unterminated(UnterminatedKind::MultilineComment, 1)]);
    errors!(" /* a /", [LexError::Unterminated(UnterminatedKind::MultilineComment, 1)]);
    errors!(" /* a // b", [LexError::Unterminated(UnterminatedKind::MultilineComment, 1)]);
    errors!(" /* a // b\n", [LexError::Unterminated(UnterminatedKind::MultilineComment, 1)]);

    // with line continuations
    let src =
r"
a
 /\
/__\
    commentaire
b
";
    tokens!(src, [(name("a"), 1..2), (name("b"), 28..29)]);

    let src =
r"
a/\
* blabla \
blabla *\
/b
";
    tokens!(src, [(name("a"), 1..2), (name("b"), 27..28)]);
}

#[test]
fn char() {
    tokens!("'a'", [(Char(Encoding::Ordinary, 'a' as u32, None), 0..3)]);
    tokens!("'+'", [(Char(Encoding::Ordinary, '+' as u32, None), 0..3)]);
    // with UCN in basic charset
    tokens!(r"'\u0061'", [(Char(Encoding::Ordinary, 'a' as u32, None), 0..8)]);
    lexeme!(r"'\u0061'", "'a'");

    // with prefix
    tokens!("u8'a'", [(Char(Encoding::Utf8, 'a' as u32, None), 0..5)]);
    tokens!("u'a'", [(Char(Encoding::Utf16, 'a' as u32, None), 0..4)]);
    tokens!("U'a'", [(Char(Encoding::Utf32, 'a' as u32, None), 0..4)]);
    tokens!("L'a'", [(Char(Encoding::Wide, 'a' as u32, None), 0..4)]);
    // not a prefix
    tokens!("u 'a'", [
        (name("u"), 0..1),
        (Char(Encoding::Ordinary, 'a' as u32, None), 2..5),
    ]);
    tokens!("a'a'", [
        (name("a"), 0..1),
        (Char(Encoding::Ordinary, 'a' as u32, None), 1..4),
    ]);

    // non ascii / out of range
    errors!("'😀'", [LexError::Char(CharError::Unmappable, 0..6)]);
    errors!("u8'😀'", [LexError::Char(CharError::Unmappable, 0..8)]);
    tokens!("u'ÿ'", [(Char(Encoding::Utf16, 'ÿ' as u32, None), 0..5)]);
    errors!("u'😀'", [LexError::Char(CharError::Unmappable, 0..7)]);
    tokens!("U'😀'", [(Char(Encoding::Utf32, '😀' as u32, None), 0..7)]);
    errors!("'é'", [LexError::Char(CharError::Unmappable, 0..4)]);
    tokens!("u'é'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..5)]);
    tokens!("U'é'", [(Char(Encoding::Utf32, 'é' as u32, None), 0..5)]);
    // ÿ == 0xFF == 255 mais les numeric escapes ont le droit d'utiliser toute
    // la capacité y compris si ce n'est pas un code point Unicode valide
    errors!("'ÿ'", [LexError::Char(CharError::Unmappable, 0..4)]);
    tokens!(r"'\xFF'", [(Char(Encoding::Ordinary, 'ÿ' as u32, None), 0..6)]);

    // multichar
    tokens!("'abcd'", [(Multichar(1633837924, None), 0..6)]);
    errors!("'abcde'", [LexError::Char(CharError::TooManyChars, 0..7)]);
    errors!(r"u8'ab'", [LexError::Char(CharError::MulticharPrefix, 0..6)]);
    errors!(r"u'ab'", [LexError::Char(CharError::MulticharPrefix, 0..5)]);
    errors!(r"U'ab'", [LexError::Char(CharError::MulticharPrefix, 0..5)]);
    errors!(r"L'ab'", [LexError::Char(CharError::MulticharPrefix, 0..5)]);
    tokens!("a'ab'", [
        (name("a"), 0..1),
        (Multichar(24930, None), 1..5),
    ]);
    // invalid chars
    // todo: more precise location
    errors!(r"'aéô'", [
        LexError::Char(CharError::NonAsciiInMultichar, 0..7),
        LexError::Char(CharError::NonAsciiInMultichar, 0..7),
    ]);
    // with multiple numeric escapes
    tokens!("'\\75\\76'", [(Multichar(15678, None), 0..8)]);
    // too many chars et invalid escape
    // todo: peut-être qu'on voudrait afficher l'erreur too many chars car
    // on voit bien qu'à part l'escape invalide il y avait 5 chars valides et donc
    // dans tous les cas ça dépasse le nombre de chars autorisés dans un multichar
    errors!("'abcde\\xFFFF'", [LexError::NumericEscapeOutOfRange(6..7)]);
    // l'escape sequence est invalide donc on la considère caractère par caractère
    // mais on n'affiche pas l'erreur too many chars pour autant car on considère
    // que le mec n'a pas voulu faire un multichar (juste un char, qui se trouve
    // être invalide)
    errors!("'\\xFFFF'", [LexError::NumericEscapeOutOfRange(1..2)]);
    // todo: on n'affiche pas l'erreur multichar prefix mais peut-être qu'on devrait?
    errors!("u8'\\75\\xFFFF'", [LexError::NumericEscapeOutOfRange(6..7)]);

    // empty
    // quand on rencontre un caractère vide, on note l'erreur et on le remplace
    // par un '\0' pour pouvoir quand même continuer le lexing
    // todo: peut-être qu'il faut pas faire comme ça ?
    tokens_and_errors!("''",
        [(Char(Encoding::Ordinary, '\0' as u32, None), 0..2)],
        [LexError::Char(CharError::Empty, 0..2)]
    );
    errors!("u8''", [LexError::Char(CharError::Empty, 0..4)]);
    errors!("u''", [LexError::Char(CharError::Empty, 0..3)]);
    errors!("U''", [LexError::Char(CharError::Empty, 0..3)]);
    errors!("L''", [LexError::Char(CharError::Empty, 0..3)]);
    errors!(" ''", [LexError::Char(CharError::Empty, 1..3)]);

    // out of range octal escape sequences
    tokens!(r"'\377'", [(Char(Encoding::Ordinary, 255, None), 0..6)]);
    errors!(r"'\400'", [LexError::NumericEscapeOutOfRange(1..2)]);
    tokens!(r"u8'\377'", [(Char(Encoding::Utf8, 255, None), 0..8)]);
    errors!(r"u8'\400'", [LexError::NumericEscapeOutOfRange(3..4)]);
    tokens!(r"L'\o{177777}'", [(Char(Encoding::Wide, 65_535, None), 0..13)]);
    errors!(r"L'\o{200000}'", [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r"u'\o{177777}'", [(Char(Encoding::Utf16, 65_535, None), 0..13)]);
    errors!(r"u'\o{200000}'", [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r"U'\o{37777777777}'", [(Char(Encoding::Utf32, 4_294_967_295, None), 0..18)]);
    errors!(r"U'\o{40000000000}'", [LexError::NumericEscapeOutOfRange(2..3)]);

    // out of range hex escape sequences
    tokens!(r"'\xFF'", [(Char(Encoding::Ordinary, 255, None), 0..6)]);
    errors!(r"'\x100'", [LexError::NumericEscapeOutOfRange(1..2)]);
    tokens!(r"u8'\xFF'", [(Char(Encoding::Utf8, 255, None), 0..8)]);
    errors!(r"u8'\x100'", [LexError::NumericEscapeOutOfRange(3..4)]);
    tokens!(r"L'\xFFFF'", [(Char(Encoding::Wide, 65_535, None), 0..9)]);
    errors!(r"L'\x10000'", [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r"u'\xFFFF'", [(Char(Encoding::Utf16, 65_535, None), 0..9)]);
    errors!(r"u'\x10000'", [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r"U'\xFFFFFFFF'", [(Char(Encoding::Utf32, 4_294_967_295, None), 0..13)]);
    errors!(r"U'\x100000000'", [LexError::NumericEscapeOutOfRange(2..3)]);

    // invalid escape sequence (on note l'erreur mais on interprète caractère par
    // caractère pour pouvoir continuer le lexing)
    tokens_and_errors!(r"'\q'",
        [(Multichar(23665, None), 0..4)],
        [LexError::Escape(EscapeError::UnknownEscape, 1..2)]
    );

    // UCN
    // `\u0027` == `'` mais il faut pas les confondre (:
    tokens!(r"'\u0027'", [(Char(Encoding::Ordinary, '\'' as u32, None), 0..8)]);
    lexeme!(r"'\u0027'", r"'''");
    errors!(r"'\u0027", [LexError::Unterminated(UnterminatedKind::Char, 0)]);
    // ici on détecte bien que le `\u0027` n'est pas valide (car en dehors d'un
    // char ou str) mais on considère quand même au final que c'est un `'`,
    // pour pouvoir continuer le lexing
    // on se retrouve donc avec le caractère 'a' pour la suite du lexing
    tokens_and_errors!(r"\u0027a'",
        [(Char(Encoding::Ordinary, 'a' as u32, None), 0..8)],
        [LexError::UnexpectedBasicUcn { c: '\'', is_control: false, span: 0..1 }]
    );

    // user-defined suffix
    tokens!("'a'a", [(Char(Encoding::Ordinary, 'a' as u32, Some(NonZeroU32::new(3).unwrap())), 0..4)]);
    tokens!("'a'_abc", [(Char(Encoding::Ordinary, 'a' as u32, Some(NonZeroU32::new(3).unwrap())), 0..7)]);
    tokens!("'a'_abc+", [
        (Char(Encoding::Ordinary, 'a' as u32, Some(NonZeroU32::new(3).unwrap())), 0..7),
        (Plus, 7..8),
    ]);
    tokens!("'abcd'a", [(Multichar(1633837924, Some(NonZeroU32::new(6).unwrap())), 0..7)]);
    // with prefix
    tokens!("u8'a'_abc", [(Char(Encoding::Utf8, 'a' as u32, Some(NonZeroU32::new(5).unwrap())), 0..9)]);
    // with UCN
    tokens!(r"'\u0061'abc", [(Char(Encoding::Ordinary, 'a' as u32, Some(NonZeroU32::new(3).unwrap())), 0..11)]);
    tokens!(r"'a'\u00E9bc", [(Char(Encoding::Ordinary, 'a' as u32, Some(NonZeroU32::new(3).unwrap())), 0..11)]);
    lexeme!(r"'a'\u00E9bc", "'a'ébc");
    // not a suffix
    tokens!(r"'a'+", [
        (Char(Encoding::Ordinary, 'a' as u32, None), 0..3),
        (Plus, 3..4),
    ]);
    // unterminated (on considère que c'est '\0' pour pouvoir continuer le lexing)
    tokens_and_errors!("'",
        [(Char(Encoding::Ordinary, '\0' as u32, None), 0..1)],
        [LexError::Unterminated(UnterminatedKind::Char, 0)]
    );
    errors!("'a", [LexError::Unterminated(UnterminatedKind::Char, 0)]);
    errors!("u8'a", [LexError::Unterminated(UnterminatedKind::Char, 0)]);
    errors!(" '", [LexError::Unterminated(UnterminatedKind::Char, 1)]);
    // unterminated and too many chars (the only error is "unterminated")
    errors!("'abcdef", [LexError::Unterminated(UnterminatedKind::Char, 0)]);

    // on peut pas mettre de newline dans un char
    errors!("'a\n'", [
        LexError::Unterminated(UnterminatedKind::Char, 0),
        LexError::Unterminated(UnterminatedKind::Char, 3),
    ]);
    errors!("'\n'", [
        LexError::Unterminated(UnterminatedKind::Char, 0),
        LexError::Unterminated(UnterminatedKind::Char, 2),
    ]);
    errors!("'\r\n'", [
        LexError::Unterminated(UnterminatedKind::Char, 0),
        LexError::Unterminated(UnterminatedKind::Char, 3),
    ]);
    errors!("'\r'", [
        LexError::Unterminated(UnterminatedKind::Char, 0),
        LexError::Unterminated(UnterminatedKind::Char, 2),
    ]);
    // newline avec un suffixe (on considère bien que le suffixe ne fait pas
    // partie de la chaîne, car le char s'arrête au newline)
    tokens_and_errors!("'a\n_abc",
        [
            (Char(Encoding::Ordinary, 'a' as u32, None), 0..2),
            (name("_abc"), 3..7),
        ],
        [LexError::Unterminated(UnterminatedKind::Char, 0)]
    );

    // with line continuations
    tokens!("u\\ \n8\\\n'\\\na\\\n'\\\n_a\\\nbc", [(Char(Encoding::Utf8, 'a' as u32, Some(NonZeroU32::new(5).unwrap())), 0..22)]);
    lexeme!("u\\ \n8\\\n'\\\na\\\n'\\\n_a\\\nbc", "u8'a'_abc");
}

#[test]
fn str() {
    tokens!("\"\"", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8(""), None), 0..2)]);
    tokens!("\"abc\"", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("abc"), None), 0..5)]);
    tokens!("\"ab c+é\\n😀\"", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("ab c+é\n😀"), None), 0..15)]);
    // with line continuations and UCN (in basic charset)
    tokens!("\"a\\\nb\"", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("ab"), None), 0..6)]);
    tokens!(r#""\u0061""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("a"), None), 0..8)]);

    // with prefix
    tokens!("L\"abc\"", [(Str(StrKind::NonRaw, Encoding::Wide, to_utf16("abc"), None), 0..6)]);
    tokens!("u8\"abc\"", [(Str(StrKind::NonRaw, Encoding::Utf8, to_utf8("abc"), None), 0..7)]);
    tokens!("u\"abc\"", [(Str(StrKind::NonRaw, Encoding::Utf16, to_utf16("abc"), None), 0..6)]);
    tokens!("U\"abc\"", [(Str(StrKind::NonRaw, Encoding::Utf32, to_utf32("abc"), None), 0..6)]);
    // not a prefix
    tokens!(r#"u8 "abc""#, [
        (name("u8"), 0..2),
        (Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("abc"), None), 3..8),
    ]);
    tokens!(r#"a"abc""#, [
        (name("a"), 0..1),
        (Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("abc"), None), 1..6),
    ]);

    // user-defined suffix
    tokens!("\"salut\"abc", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(7).unwrap())), 0..10)]);
    tokens!("\"salut\"_abc", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(7).unwrap())), 0..11)]);
    tokens!("\"salut\"_abc+", [
        (Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(7).unwrap())), 0..11),
        (Plus, 11..12),
    ]);
    // with UCN
    tokens!("\"s\\u0061lut\"abc", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(7).unwrap())), 0..15)]);
    tokens!("\"salut\"\\u00E9bc", [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(7).unwrap())), 0..15)]);
    lexeme!("\"salut\"\\u00E9bc", "\"salut\"ébc");
    // not a suffix
    tokens!("\"abc\"2", [
        (Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("abc"), None), 0..5),
        (Number, 5..6)
    ]);

    // test encoding
    tokens!("\"😀\"", [(Str(StrKind::NonRaw, Encoding::Ordinary, ByteString(vec![0xF0, 0x9F, 0x98, 0x80, 0]), None), 0..6)]);
    tokens!("L\"😀\"", [(Str(StrKind::NonRaw, Encoding::Wide, ByteString(vec![0x3D, 0xD8, 0x00, 0xDE, 0]), None), 0..7)]);
    tokens!("u8\"😀\"", [(Str(StrKind::NonRaw, Encoding::Utf8, ByteString(vec![0xF0, 0x9F, 0x98, 0x80, 0]), None), 0..8)]);
    tokens!("u\"😀\"", [(Str(StrKind::NonRaw, Encoding::Utf16, ByteString(vec![0x3D, 0xD8, 0x00, 0xDE, 0]), None), 0..7)]);
    tokens!("U\"😀\"", [(Str(StrKind::NonRaw, Encoding::Utf32, ByteString(vec![0x00, 0xF6, 0x01, 0x00, 0]), None), 0..7)]);
    // with numeric escape
    tokens!("\"\\xAA\"", [(Str(StrKind::NonRaw, Encoding::Ordinary, ByteString(vec![0xAA, 0]), None), 0..6)]);
    tokens!("L\"\\xAABB\"", [(Str(StrKind::NonRaw, Encoding::Wide, ByteString(vec![0xBB, 0xAA, 0]), None), 0..9)]);
    tokens!("u8\"\\xAA\"", [(Str(StrKind::NonRaw, Encoding::Utf8, ByteString(vec![0xAA, 0]), None), 0..8)]);
    tokens!("u\"\\xAABB\"", [(Str(StrKind::NonRaw, Encoding::Utf16, ByteString(vec![0xBB, 0xAA, 0]), None), 0..9)]);
    tokens!("U\"\\xAABBCCDD\"", [(Str(StrKind::NonRaw, Encoding::Utf32, ByteString(vec![0xDD, 0xCC, 0xBB, 0xAA, 0]), None), 0..13)]);

    // invalid escape sequence (on note l'erreur mais on interprète caractère par
    // caractère pour pouvoir continuer le lexing)
    tokens_and_errors!(r#""ab\qc""#,
        [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8(r"ab\qc"), None), 0..7)],
        [LexError::Escape(EscapeError::UnknownEscape, 3..4)]
    );

    // unterminated
    errors!(" \"", [LexError::Unterminated(UnterminatedKind::Str, 1)]);
    errors!("\"a", [LexError::Unterminated(UnterminatedKind::Str, 0)]);
    // le token Str a quand même la valeur des caractères qu'on a trouvé jusque là
    tokens_and_errors!("\"abcd",
        [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("abcd"), None), 0..5)],
        [LexError::Unterminated(UnterminatedKind::Str, 0)]
    );

    // `\u0022` == `"` mais il faut pas les confondre
    tokens!(r#""\u0022""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\""), None), 0..8)]);
    lexeme!(r#""\u0022""#, r#"""""#);
    errors!(r#""\u0022"#, [LexError::Unterminated(UnterminatedKind::Str, 0)]);
    // ici on détecte bien que le `\u0022` n'est pas valide mais on considère
    // quand même que c'est un `"` pour continuer le lexing
    tokens_and_errors!(r#"\u0022a""#,
        [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("a"), None), 0..8)],
        [LexError::UnexpectedBasicUcn { c: '\"', is_control: false, span: 0..1 }]
    );

    // ne peut pas contenir de newline
    errors!("\"\n\"", [
        LexError::Unterminated(UnterminatedKind::Str, 0),
        LexError::Unterminated(UnterminatedKind::Str, 2),
    ]);
    errors!("\"\r\n\"", [
        LexError::Unterminated(UnterminatedKind::Str, 0),
        LexError::Unterminated(UnterminatedKind::Str, 3),
    ]);
    errors!("\"\r\"", [
        LexError::Unterminated(UnterminatedKind::Str, 0),
        LexError::Unterminated(UnterminatedKind::Str, 2),
    ]);
    // newline avec un suffixe (on considère bien que le suffixe ne fait pas
    // partie de la chaîne, car la chaîne s'arrête au newline)
    tokens_and_errors!("\"salut\n_abc",
        [
            (Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("salut"), None), 0..6),
            (name("_abc"), 7..11),
        ],
        [LexError::Unterminated(UnterminatedKind::Str, 0)]
    );

    let u8_bytes = |v: u8| ByteString([&v.to_le_bytes()[..], &[0]].concat());
    let u16_bytes = |v: u16| ByteString([&v.to_le_bytes()[..], &[0]].concat());
    let u32_bytes = |v: u32| ByteString([&v.to_le_bytes()[..], &[0]].concat());

    // out of range octal escape sequences
    tokens!(r#""\377""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, u8_bytes(255), None), 0..6)]);
    errors!(r#""\400""#, [LexError::NumericEscapeOutOfRange(1..2)]);
    tokens!(r#"u8"\377""#, [(Str(StrKind::NonRaw, Encoding::Utf8, u8_bytes(255), None), 0..8)]);
    errors!(r#"u8"\400""#, [LexError::NumericEscapeOutOfRange(3..4)]);
    tokens!(r#"L"\o{177777}""#, [(Str(StrKind::NonRaw, Encoding::Wide, u16_bytes(65_535), None), 0..13)]);
    errors!(r#"L"\o{200000}""#, [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r#"u"\o{177777}""#, [(Str(StrKind::NonRaw, Encoding::Utf16, u16_bytes(65_535), None), 0..13)]);
    errors!(r#"u"\o{200000}""#, [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r#"U"\o{37777777777}""#, [(Str(StrKind::NonRaw, Encoding::Utf32, u32_bytes(4_294_967_295), None), 0..18)]);
    errors!(r#"U"\o{40000000000}""#, [LexError::NumericEscapeOutOfRange(2..3)]);

    // out of range hex escape sequences
    tokens!(r#""\xFF""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, u8_bytes(255), None), 0..6)]);
    errors!(r#""\x100""#, [LexError::NumericEscapeOutOfRange(1..2)]);
    tokens!(r#"u8"\xFF""#, [(Str(StrKind::NonRaw, Encoding::Utf8, u8_bytes(255), None), 0..8)]);
    errors!(r#"u8"\x100""#, [LexError::NumericEscapeOutOfRange(3..4)]);
    tokens!(r#"L"\xFFFF""#, [(Str(StrKind::NonRaw, Encoding::Wide, u16_bytes(65_535), None), 0..9)]);
    errors!(r#"L"\x10000""#, [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r#"u"\xFFFF""#, [(Str(StrKind::NonRaw, Encoding::Utf16, u16_bytes(65_535), None), 0..9)]);
    errors!(r#"u"\x10000""#, [LexError::NumericEscapeOutOfRange(2..3)]);
    tokens!(r#"U"\xFFFFFFFF""#, [(Str(StrKind::NonRaw, Encoding::Utf32, u32_bytes(4_294_967_295), None), 0..13)]);
    errors!(r#"U"\x100000000""#, [LexError::NumericEscapeOutOfRange(2..3)]);

    // with line continuations and UCN
    tokens!("u\\\n8\"a\\\nb\\u00E9\\\n\"\\\n_a\\u00E9bc", [(Str(StrKind::NonRaw, Encoding::Utf8, to_utf8("abé"), Some(NonZeroU32::new(8).unwrap())), 0..30)]);
    lexeme!("u\\\n8\"a\\\nb\\u00E9\\\n\"\\\n_a\\u00E9bc", "u8\"abé\"_aébc");
}

#[test]
fn raw_str() {
    tokens!("R\"()\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(""), None), 0..5)]);
    tokens!("R\"(abc)\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc"), None), 0..8)]);
    tokens!("R\"())\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(")"), None), 0..6)]);
    tokens!("R\"(())\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("()"), None), 0..7)]);
    tokens!("R\"(è'\n\"à\"😀\\)\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("è'\n\"à\"😀\\"), None), 0..18)]);

    // line continuations and escape sequences are not recognized
    tokens!("R\"(a\\\nb)\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("a\\\nb"), None), 0..9)]);
    tokens!(r#"R"(\u0061)""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(r"\u0061"), None), 0..11)]);
    tokens!(r#"R"(\n)""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(r"\n"), None), 0..7)]);
    tokens!(r#"R"(\0)""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(r"\0"), None), 0..7)]);
    tokens!(r#"R"(\o{77})""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(r"\o{77}"), None), 0..11)]);
    tokens!(r#"R"(\xFF)""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(r"\xFF"), None), 0..9)]);
    tokens!(r#"R"(\x{FF})""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(r"\x{FF}"), None), 0..11)]);

    // with delim
    tokens!("R\"delim()delim\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(""), None), 0..15)]);
    tokens!("R\"delim(abc)delim\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc"), None), 0..18)]);
    tokens!("R\"delim(abc)\")delim\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc)\""), None), 0..20)]);
    tokens!("R\"delim(delim)delim\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("delim"), None), 0..20)]);
    tokens!("R\"del+im(abc)del+im\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc"), None), 0..20)]);
    tokens!("R\"delim()delim)delim\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(")delim"), None), 0..21)]);
    tokens!("R\"delim()deli)delim\"", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(")deli"), None), 0..20)]);

    // invalid char in delim
    // (dans ce cas on cherche la fin de la chaîne (caractère `"`) et on considère
    // qu'elle vaut `""`)
    // todo: peut-être qu'on peut faire mieux ?
    tokens_and_errors!("R\"de\nim(abc)de\nim\"",
        [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8(""), None), 0..18)],
        [LexError::Str(StrError::InvalidCharInDelim, 4..5)]
    );
    errors!("R\"de\r\nim(abc)de\r\nim\"", [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!("R\"de\rim(abc)de\rim\"", [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!("R\"de\tim(abc)de\tim\"", [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!("R\"de\u{B}im(abc)de\u{B}im\"", [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!("R\"de\u{C}im(abc)de\u{C}im\"", [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!(r#"R"de im(abc)de im""#, [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!(r#"R"de\im(abc)de\im""#, [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!(r#"R"de)im(abc)de)im""#, [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    errors!(r#"R"délim(abc)délim""#, [LexError::Str(StrError::InvalidCharInDelim, 3..4)]);
    errors!(r#"R"d🤡lim(abc)d🤡lim""#, [LexError::Str(StrError::InvalidCharInDelim, 3..4)]);
    // line continuations and UCN are not allowed in delim
    errors!("R\"d\\\nelim(abc)d\\\nelim\"", [LexError::Str(StrError::InvalidCharInDelim, 3..4)]);
    errors!(r#"R"de\u0061im(abc)de\u0061im""#, [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);
    // delim invalide mais avec un `"` en plein milieu de la chaîne, on considère
    // donc que c'est la fin de la chaîne même si ce n'est pas le cas (on considère
    // qu'on ne peut pas utiliser le délim pour trouver la fin vu qu'il est invalide
    // donc on fait au mieux)
    tokens_and_errors!(r#"R"de im(abc")de im"#,
        [
            (Str(StrKind::Raw, Encoding::Ordinary, to_utf8(""), None), 0..12),
            (ParenR, 12..13),
            (name("de"), 13..15),
            (name("im"), 16..18),
        ],
        [LexError::Str(StrError::InvalidCharInDelim, 4..5)]
    );
    // invalid char in delim and unterminated
    errors!(r#"R"de\im(abc)de\im"#, [LexError::Str(StrError::InvalidCharInDelim, 4..5)]);

    // too many chars in delim
    tokens!(r#"R"abcdefghijklmno(abc)abcdefghijklmno""#, [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc"), None), 0..38)]);
    // on note l'erreur mais on reconnaît quand même la chaîne pour pouvoir continuer le lexing
    tokens_and_errors!(r#"R"abcdefghijklmnop(abc)abcdefghijklmnop""#,
        [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc"), None), 0..40)],
        [LexError::Str(StrError::TooManyCharsInDelim, 2..18)]
    );

    // without `(` and `)`
    errors!("R\"salut\"", [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!("R\"(salut\"", [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!("R\"salut)\"", [LexError::Str(StrError::InvalidCharInDelim, 7..8)]);

    // with prefix
    tokens!("LR\"(abc)\"", [(Str(StrKind::Raw, Encoding::Wide, to_utf16("abc"), None), 0..9)]);
    tokens!("u8R\"(abc)\"", [(Str(StrKind::Raw, Encoding::Utf8, to_utf8("abc"), None), 0..10)]);
    tokens!("uR\"(abc)\"", [(Str(StrKind::Raw, Encoding::Utf16, to_utf16("abc"), None), 0..9)]);
    tokens!("UR\"(abc)\"", [(Str(StrKind::Raw, Encoding::Utf32, to_utf32("abc"), None), 0..9)]);
    // not a prefix
    tokens!("Ru8\"(abc)\"", [
        (name("Ru8"), 0..3),
        (Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("(abc)"), None), 3..10),
    ]);

    // user-defined suffix
    tokens!("R\"(salut)\"abc", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(10).unwrap())), 0..13)]);
    tokens!("R\"(salut)\"_abc", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(10).unwrap())), 0..14)]);
    tokens!("R\"(salut)\"_abc+", [
        (Str(StrKind::Raw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(10).unwrap())), 0..14),
        (Plus, 14..15)
    ]);
    // with UCN
    tokens!("R\"(salut)\"\\u00E9bc", [(Str(StrKind::Raw, Encoding::Ordinary, to_utf8("salut"), Some(NonZeroU32::new(10).unwrap())), 0..18)]);
    lexeme!("R\"(salut)\"\\u00E9bc", r#"R"(salut)"ébc"#);
    // not a suffix
    tokens!("R\"(abc)\"2", [
        (Str(StrKind::Raw, Encoding::Ordinary, to_utf8("abc"), None), 0..8),
        (Number, 8..9)
    ]);

    // test encoding
    tokens!("R\"(😀)\"", [(Str(StrKind::Raw, Encoding::Ordinary, ByteString(vec![0xF0, 0x9F, 0x98, 0x80, 0]), None), 0..9)]);
    tokens!("LR\"(😀)\"", [(Str(StrKind::Raw, Encoding::Wide, ByteString(vec![0x3D, 0xD8, 0x00, 0xDE, 0]), None), 0..10)]);
    tokens!("u8R\"(😀)\"", [(Str(StrKind::Raw, Encoding::Utf8, ByteString(vec![0xF0, 0x9F, 0x98, 0x80, 0]), None), 0..11)]);
    tokens!("uR\"(😀)\"", [(Str(StrKind::Raw, Encoding::Utf16, ByteString(vec![0x3D, 0xD8, 0x00, 0xDE, 0]), None), 0..10)]);
    tokens!("UR\"(😀)\"", [(Str(StrKind::Raw, Encoding::Utf32, ByteString(vec![0x00, 0xF6, 0x01, 0x00, 0]), None), 0..10)]);

    // unterminated
    // without closing quote
    errors!(r#" R""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 1)]);
    errors!(r#"R""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!(r#"R"("#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!(r#"R"delim"#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!(r#"R"delim("#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    errors!(r#"R"delim(abc"#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    errors!(r#"R"delim(abc)"#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    // with closing quote
    errors!(r#"R"""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!(r#"R"(""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!(r#"R"delim""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);
    errors!(r#"R"delim(""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    errors!(r#"R"delim(abc""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    errors!(r#"R"delim(abc)""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    // begin and end delim not matching
    errors!(r#"R"delim(abc)lol""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    errors!(r#"R"delim(abc)delim2""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: Some("delim".into()) }, 0)]);
    // with too many chars in delim (the only error is "unterminated")
    errors!(r#"R"abcdefghijklmnopqrs""#, [LexError::Unterminated(UnterminatedKind::RawStr { delim: None }, 0)]);

    // with line continuations
    tokens!("u\\\n8R\\\n\"delim(a\\ \né\\\nc)delim\"_a\\ \nb\\\nc", [(Str(StrKind::Raw, Encoding::Utf8, to_utf8("a\\ \né\\\nc"), Some(NonZeroU32::new(26).unwrap())), 0..39)]);
    lexeme!("u\\\n8R\\\n\"delim(a\\ \né\\\nc)delim\"_a\\ \nb\\\nc", "u8R\"delim(a\\ \né\\\nc)delim\"_abc");
}

#[test]
fn simple_escape_seq() {
    // in char
    tokens!(r"'\n'", [(Char(Encoding::Ordinary, '\n' as u32, None), 0..4)]);
    tokens!(r"'\t'", [(Char(Encoding::Ordinary, '\t' as u32, None), 0..4)]);
    tokens!(r"'\v'", [(Char(Encoding::Ordinary, '\u{B}' as u32, None), 0..4)]);
    tokens!(r"'\a'", [(Char(Encoding::Ordinary, '\u{7}' as u32, None), 0..4)]);
    tokens!(r"'\b'", [(Char(Encoding::Ordinary, '\u{8}' as u32, None), 0..4)]);
    tokens!(r"'\r'", [(Char(Encoding::Ordinary, '\r' as u32, None), 0..4)]);
    tokens!(r"'\f'", [(Char(Encoding::Ordinary, '\u{C}' as u32, None), 0..4)]);
    tokens!(r"'\\'", [(Char(Encoding::Ordinary, '\\' as u32, None), 0..4)]);
    tokens!(r"'\?'", [(Char(Encoding::Ordinary, '?' as u32, None), 0..4)]);
    tokens!(r"'\''", [(Char(Encoding::Ordinary, '\'' as u32, None), 0..4)]);
    tokens!(r#"'\"'"#, [(Char(Encoding::Ordinary, '"' as u32, None), 0..4)]);

    // in str
    tokens!(r#""\n""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\n"), None), 0..4)]);
    tokens!(r#""\t""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\t"), None), 0..4)]);
    tokens!(r#""\v""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\u{B}"), None), 0..4)]);
    tokens!(r#""\a""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\u{7}"), None), 0..4)]);
    tokens!(r#""\b""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\u{8}"), None), 0..4)]);
    tokens!(r#""\r""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\r"), None), 0..4)]);
    tokens!(r#""\f""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\u{C}"), None), 0..4)]);
    tokens!(r#""\\""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\\"), None), 0..4)]);
    tokens!(r#""\?""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("?"), None), 0..4)]);
    tokens!(r#""\'""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\'"), None), 0..4)]);
    tokens!(r#""\"""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("\""), None), 0..4)]);

    // invalid
    errors!(r"'\z'", [LexError::Escape(EscapeError::UnknownEscape, 1..2)]);

    // outside char and str (ce n'est pas encore une erreur, juste un token Unknown)
    tokens!(r"\n", [(Unknown, 0..1), (name("n"), 1..2)]);

    // on ne peut utiliser des UCN pour former une escape sequence
    // `\u006E` = `n`
    tokens!(r#""\\u006E""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8(r"\u006E"), None), 0..9)]);

    // with line continuations
    tokens!("'\\\\ \nn'", [(Char(Encoding::Ordinary, '\n' as u32, None), 0..7)]);
    lexeme!("'\\\\ \nn'", r"'\n'");
}

#[test]
fn numeric_escape_seq() {
    // octal escape sequence
    tokens!(r"'\0'", [(Char(Encoding::Ordinary, 0, None), 0..4)]);
    tokens!(r"'\1'", [(Char(Encoding::Ordinary, 1, None), 0..4)]);
    tokens!(r"'\12'", [(Char(Encoding::Ordinary, 10, None), 0..5)]);
    tokens!(r"'\123'", [(Char(Encoding::Ordinary, 83, None), 0..6)]);
    // (le 4 ne fait pas partie de l'escape)
    tokens!(r"'\1234'", [(Multichar(21300, None), 0..7)]);
    tokens!(r"'\18'", [(Multichar(312, None), 0..5)]);
    tokens!(r"'\1a'", [(Multichar(353, None), 0..5)]);
    tokens!(r"'\12a'", [(Multichar(2657, None), 0..6)]);
    tokens!(r"'\123a'", [(Multichar(21345, None), 0..7)]);
    tokens!(r"'\o{12}'", [(Char(Encoding::Ordinary, 10, None), 0..8)]);
    tokens!(r"'\o{12}3'", [(Multichar(2611, None), 0..9)]);
    // in str
    tokens!(r#""\123a""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("Sa"), None), 0..7)]);
    tokens!(r#""\o{123}3""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("S3"), None), 0..10)]);
    // consecutive
    tokens!(r#""\123\122""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("SR"), None), 0..10)]);
    tokens!(r#""\o{123}\o{122}""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("SR"), None), 0..16)]);

    // hex escape sequence
    tokens!(r"'\x1a'", [(Char(Encoding::Ordinary, 0x1a, None), 0..6)]);
    tokens!(r"'\x{1a}'", [(Char(Encoding::Ordinary, 0x1a, None), 0..8)]);
    tokens!(r"'\x{1}a'", [(Multichar(353, None), 0..8)]);
    tokens!(r"'\x1g'", [(Multichar(359, None), 0..6)]);
    // in str
    tokens!(r#""\x4Dg""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("Mg"), None), 0..7)]);
    tokens!(r#""\x{4D}3""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("M3"), None), 0..9)]);
    // consecutive
    tokens!(r#""\x4D\x4E""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("MN"), None), 0..10)]);
    tokens!(r#""\x{4D}\x{4E}""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("MN"), None), 0..14)]);

    // incomplete
    // todo: more precise locations
    errors!(r"'\o'", [LexError::Escape(EscapeError::ExpectedOpenBrace, 1..2)]);
    errors!(r"'\o{'", [LexError::Escape(EscapeError::NoCloseBrace, 1..2)]);
    errors!(r"'\x'", [LexError::Escape(EscapeError::ExpectedOpenBraceOrHexDigit, 1..2)]);
    errors!(r"'\xg'", [LexError::Escape(EscapeError::ExpectedOpenBraceOrHexDigit, 1..2)]);
    errors!(r"'\x{'", [LexError::Escape(EscapeError::NoCloseBrace, 1..2)]);
    // empty braces
    errors!(r"'\o{}'", [LexError::Escape(EscapeError::EmptyBraces, 1..2)]);
    errors!(r"'\x{}'", [LexError::Escape(EscapeError::EmptyBraces, 1..2)]);

    // invalid digit
    errors!(r"'\o{128}'", [LexError::Escape(EscapeError::InvalidDigitInBraces { base: 8 }, 1..2)]);
    errors!(r"'\o{12A}'", [LexError::Escape(EscapeError::InvalidDigitInBraces { base: 8 }, 1..2)]);
    errors!(r"'\x{ABS}'", [LexError::Escape(EscapeError::InvalidDigitInBraces { base: 16 }, 1..2)]);
    errors!(r"'\x{A+B}'", [LexError::Escape(EscapeError::InvalidDigitInBraces { base: 16 }, 1..2)]);

    // on ne peut pas utiliser des UCN pour former une escape sequence
    // `\u0033` == `3` donc teste que ce n'est pas `\613` ni `\x313`
    tokens!(r#""\61\u0033""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("13"), None), 0..11)]);
    tokens!(r#""\x31\u0033""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("13"), None), 0..12)]);
    // `\u007B` == `{`
    tokens!(r"'\o{123}'", [(Char(Encoding::Ordinary, 83, None), 0..9)]);
    errors!(r"'\o\u007B123}'", [LexError::Escape(EscapeError::ExpectedOpenBrace, 1..2)]);
    tokens!(r"'\x{0}'", [(Char(Encoding::Ordinary, 0, None), 0..7)]);
    errors!(r"'\x\u007B0}'", [LexError::Escape(EscapeError::ExpectedOpenBraceOrHexDigit, 1..2)]);

    // outside char and str
    tokens!(r"\123", [
        (Unknown, 0..1),
        (Number, 1..4),
    ]);
    tokens!(r"\o{12}", [
        (Unknown, 0..1),
        (name("o"), 1..2),
        (BraceL, 2..3),
        (Number, 3..5),
        (BraceR, 5..6),
    ]);
    tokens!(r"\x12", [
        (Unknown, 0..1),
        (name("x12"), 1..4),
    ]);
    tokens!(r"\x{12}", [
        (Unknown, 0..1),
        (name("x"), 1..2),
        (BraceL, 2..3),
        (Number, 3..5),
        (BraceR, 5..6),
    ]);

    // with line continuations
    tokens!("'\\\\\n12\\\n3\\\n'", [(Char(Encoding::Ordinary, 83, None), 0..12)]);
    lexeme!("'\\\\\n12\\\n3\\\n'", r"'\123'");
    tokens!("'\\\\\no\\\n{\\\n1\\\n2\\\n}'", [(Char(Encoding::Ordinary, '\n' as u32, None), 0..18)]);
    lexeme!("'\\\\\no\\\n{\\\n1\\\n2\\\n}'", r"'\o{12}'");
    tokens!("'\\\\\nx\\\n{\\\n4\\\n1\\\n}'", [(Char(Encoding::Ordinary, 'A' as u32, None), 0..18)]);
    lexeme!("'\\\\\nx\\\n{\\\n4\\\n1\\\n}'", r"'\x{41}'");
    tokens!("'\\\\\nx\\\n4\\\n1\\\n'", [(Char(Encoding::Ordinary, 'A' as u32, None), 0..14)]);
    lexeme!("'\\\\\nx\\\n4\\\n1\\\n'", r"'\x41'");
}

#[test]
fn ucn() {
    tokens!(r"u'\u00E9'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..9)]);
    tokens!(r"u'\u00e9'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..9)]);
    tokens!(r"U'\U0001F600'", [(Char(Encoding::Utf32, '😀' as u32, None), 0..13)]);
    tokens!(r"U'\U0001f600'", [(Char(Encoding::Utf32, '😀' as u32, None), 0..13)]);
    tokens!(r"u'\u{E9}'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..9)]);
    tokens!(r"u'\u{e9}'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..9)]);
    tokens!(r"U'\N{GRINNING FACE}'", [(Char(Encoding::Utf32, '😀' as u32, None), 0..20)]);
    errors!(r"U'\N{grinning face}'", [LexError::Escape(EscapeError::InvalidUcnName, 2..3)]);

    // followed by something
    tokens!(r#""\u00E9A""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("éA"), None), 0..9)]);
    tokens!(r#""\U000000E9A""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("éA"), None), 0..13)]);
    tokens!(r#""\u00E91""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("é1"), None), 0..9)]);
    tokens!(r#""\U000000E91""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("é1"), None), 0..13)]);
    tokens!(r#""\u00E9ABCD""#, [(Str(StrKind::NonRaw, Encoding::Ordinary, to_utf8("éABCD"), None), 0..12)]);

    // invalid value
    tokens!(r"U'\uD7FF'", [(Char(Encoding::Utf32, 0xD7FF, None), 0..9)]);
    errors!(r"U'\uD800'", [LexError::Escape(EscapeError::InvalidUcnValue, 2..3)]);
    errors!(r"U'\uDFFF'", [LexError::Escape(EscapeError::InvalidUcnValue, 2..3)]);
    tokens!(r"U'\uE000'", [(Char(Encoding::Utf32, 0xE000, None), 0..9)]);
    tokens!(r"U'\U0010FFFF'", [(Char(Encoding::Utf32, 0x10FFFF, None), 0..13)]);
    errors!(r"U'\U00110000'", [LexError::Escape(EscapeError::InvalidUcnValue, 2..3)]);
    errors!(r"U'\UFFFFFFFF'", [LexError::Escape(EscapeError::InvalidUcnValue, 2..3)]);
    errors!(r"U'\u{FFFFFFFFFFFFFFFFFFFFFFF}'", [LexError::Escape(EscapeError::InvalidUcnValue, 2..3)]);

    // incomplete
    errors!(r"'\u000'", [LexError::Escape(EscapeError::ExpectedDigits { n: 4, base: 16 }, 1..2)]);
    errors!(r"'\up'", [LexError::Escape(EscapeError::ExpectedDigits { n: 4, base: 16 }, 1..2)]);
    errors!(r"'\Up'", [LexError::Escape(EscapeError::ExpectedDigits { n: 8, base: 16 }, 1..2)]);
    errors!(r"'\u000X'", [LexError::Escape(EscapeError::ExpectedDigits { n: 4, base: 16 }, 1..2)]);
    errors!(r"'\U0000000X'", [LexError::Escape(EscapeError::ExpectedDigits { n: 8, base: 16 }, 1..2)]);
    // incomplete named ucn
    errors!(r"\N", [LexError::Escape(EscapeError::ExpectedOpenBrace, 0..1)]);
    errors!(r"\N{", [LexError::Escape(EscapeError::NoCloseBrace, 0..1)]);
    // inside char
    errors!(r"'\N'", [LexError::Escape(EscapeError::ExpectedOpenBrace, 1..2)]);
    errors!(r"'\N{'", [LexError::Escape(EscapeError::NoCloseBrace, 1..2)]);

    // empty braces
    errors!(r"\u{}", [LexError::Escape(EscapeError::EmptyBraces, 0..1)]);
    errors!(r"\N{}", [LexError::Escape(EscapeError::EmptyBraces, 0..1)]);

    // invalid digit in braces
    errors!(r"'\u{EP}'", [LexError::Escape(EscapeError::InvalidDigitInBraces { base: 16 }, 1..2)]);

    // les UCNs en dehors d'un char ou str ne peuvent pas désigner un caractère
    // de contrôle ou un caractère du basic character set, dans ce cas on note
    // l'erreur mais on fait comme si c'était le caractère en question, pour
    // pouvoir continuer le lexing
    // todo: peut-être qu'il faut faire différemment?
    // `\u0041` == `A`
    tokens_and_errors!(r"ab\u0041c",
        [(name("abAc"), 0..9)],
        [LexError::UnexpectedBasicUcn { c: 'A', is_control: false, span: 2..3 }]
    );

    tokens_and_errors!(r"ab\u0001c",
        [(name("ab"), 0..2), (Unknown, 2..8), (name("c"), 8..9)],
        [LexError::UnexpectedBasicUcn { c: '\u{1}', is_control: true, span: 2..3 }]
    );

    tokens_and_errors!(r"\u0041",
        [(name("A"), 0..6)],
        [LexError::UnexpectedBasicUcn { c: 'A', is_control: false, span: 0..1 }]
    );

    // les ucns invalides sont interprétés caractère par caractère
    tokens_and_errors!(r"ab\uCD",
        [(name("ab"), 0..2), (Unknown, 2..3), (name("uCD"), 3..6)],
        [LexError::Escape(EscapeError::ExpectedDigits { n: 4, base: 16 }, 2..3)]
    );

    // on ne peut pas utiliser des UCN pour former un UCN
    // `\u0061` == `a`
    tokens!(r"'\u000a'", [(Char(Encoding::Ordinary, '\n' as u32, None), 0..8)]);
    errors!(r"'\u000\u0061'", [LexError::Escape(EscapeError::ExpectedDigits { n: 4, base: 16 }, 1..2)]);
    tokens!(r"'\U0000000a'", [(Char(Encoding::Ordinary, '\n' as u32, None), 0..12)]);
    errors!(r"'\U0000000\u0061'", [LexError::Escape(EscapeError::ExpectedDigits { n: 8, base: 16 }, 1..2)]);
    // `\u007B` == `{`
    tokens!(r"'\u{0000}'", [(Char(Encoding::Ordinary, '\0' as u32, None), 0..10)]);
    errors!(r"'\u\u007B0000}'", [LexError::Escape(EscapeError::ExpectedDigits { n: 4, base: 16 }, 1..2)]);
    tokens!(r"'\N{LATIN SMALL LETTER A}'", [(Char(Encoding::Ordinary, 'a' as u32, None), 0..26)]);
    errors!(r"'\N\u007BLATIN SMALL LETTER A}'", [LexError::Escape(EscapeError::ExpectedOpenBrace, 1..2)]);

    // with line continuations
    tokens!("u'\\\n\\\\\nu\\\n00\\\nE9\\\n'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..19)]);
    lexeme!("u'\\\n\\\\\nu\\\n00\\\nE9\\\n'", "u'é'");
    tokens!("U'\\\n\\\\\nU\\\n00\\\n01\\\nF6\\\n00\\\n'", [(Char(Encoding::Utf32, '😀' as u32, None), 0..27)]);
    lexeme!("U'\\\n\\\\\nU\\\n00\\\n01\\\nF6\\\n00\\\n'", "U'😀'");
    tokens!("u'\\\n\\\\\nu\\\n{\\\ne\\\n9\\\n}\\\n'", [(Char(Encoding::Utf16, 'é' as u32, None), 0..23)]);
    lexeme!("u'\\\n\\\\\nu\\\n{\\\ne\\\n9\\\n}\\\n'", "u'é'");
    tokens!("U'\\\\\nN\\\n{\\\nGRI\\\nNNIN\\\nG FACE\\\n}\\\n'", [(Char(Encoding::Utf32, '😀' as u32, None), 0..34)]);
    lexeme!("U'\\\\\nN\\\n{\\\nGRI\\\nNNIN\\\nG FACE\\\n}\\\n'", r"U'😀'");
}

#[test]
fn line_continuations() {
    tokens!("a\\\nb", [(name("ab"), 0..4)]);
    tokens!("a\\   \nb", [(name("ab"), 0..7)]);
    tokens!("a\\ \t \u{B} \u{C} \nb", [(name("ab"), 0..11)]);
    tokens!("\\  \na", [(name("a"), 4..5)]);
    tokens!("a\\\nb\\\n c", [
        (name("ab"), 0..4),
        (name("c"), 7..8),
    ]);

    // avec \r\n
    tokens!("a\\\r\nb", [(name("ab"), 0..5)]);

    // avec \r
    tokens!("a\\\rb", [(name("ab"), 0..4)]);

    // consecutive line continuations
    tokens!("a\\  \n\\   \r\\  \r\nb", [(name("ab"), 0..16)]);
    // le dernier `\` n'est pas une line continuation
    tokens!("a\\  \n\\   \r\\  b", [
        (name("a"), 0..1),
        (Unknown, 10..11),
        (name("b"), 13..14),
    ]);

    // pas une line continuation
    tokens!("a\\b", [
        (name("a"), 0..1),
        (Unknown, 1..2),
        (name("b"), 2..3),
    ]);
    tokens!("a\\   b\\   \n", [
        (name("a"), 0..1),
        (Unknown, 1..2),
        (name("b"), 5..6),
    ]);
    // fichier qui finit par un `\`
    tokens!("blabla\\", [
        (name("blabla"), 0..6),
        (Unknown, 6..7),
    ]);
}

#[test]
fn pp_number() {
    // on s'en fout un peu des tests du preprocessing number en lui-même car
    // il y a les tests des integer / floating-point literals qui sont plus exhaustifs
    tokens!("0", [(Number, 0..1)]);
    tokens!("0.", [(Number, 0..2)]);
    tokens!(".0abcd", [(Number, 0..6)]);

    // les émojis ne sont pas autorisés dans le suffixe (même règle que les identifiers)
    tokens!("123🦀", [(Number, 0..3), (Unknown, 3..7)]);
    tokens!("123_🦀", [(Number, 0..4), (Unknown, 4..8)]);

    // le `+` ne fait pas partie du suffixe
    tokens!("123_abc+", [(Number, 0..7), (Plus, 7..8)]);

    // pas un nombre
    tokens!("0x1'e+1", [(Number, 0..5), (Plus, 5..6), (Number, 6..7)]);
    tokens!("a123", [(name("a123"), 0..4)]);
    tokens!(".abcd", [(Dot, 0..1), (name("abcd"), 1..5)]);
    errors!("1'.5", [LexError::Unterminated(UnterminatedKind::Char, 1)]);

    // with line continuations and UCN
    tokens!("0\\\nx\\\n36\\\ne_\\\na\\u00E9\\\nc", [(Number, 0..24)]);
    lexeme!("0\\\nx\\\n36\\\ne_\\\na\\u00E9\\\nc", "0x36e_aéc");
}

macro_rules! assert_number_eq {
    ($number:expr, $expected:expr) => {
        // on vérifie que c'est bien un "preprocessing number"
        tokens!($number, [(Number, 0..$number.len() as u32)]);
        assert_eq!(parse_number($number), $expected);
    };
}

#[test]
fn int_literal() {
    fn number(value: i128, kind: IntLitKind, ud_suffix: Option<u32>) -> NumberLit {
        NumberLit {
            kind: NumberLitKind::Int { kind, value },
            ud_suffix: ud_suffix.map(|pos| NonZeroU32::new(pos).unwrap()),
        }
    }

    // decimal
    assert_number_eq!("1234567890", Ok(number(1234567890, IntLitKind::Int, None)));

    // binary, octal, hexadecimal
    assert_number_eq!("0b10", Ok(number(2, IntLitKind::Int, None)));
    assert_number_eq!("0B10", Ok(number(2, IntLitKind::Int, None)));
    assert_number_eq!("01234567", Ok(number(342391, IntLitKind::Int, None)));
    assert_number_eq!("0x1234567890", Ok(number(0x1234567890, IntLitKind::Long, None)));
    assert_number_eq!("0X10", Ok(number(0x10, IntLitKind::Int, None)));
    assert_number_eq!("0xAbCdEf", Ok(number(0xabcdef, IntLitKind::Int, None)));
    assert_number_eq!("0xaBcDeF", Ok(number(0xabcdef, IntLitKind::Int, None)));

    // with quotes
    assert_number_eq!("128'89'876", Ok(number(12889876, IntLitKind::Int, None)));
    assert_number_eq!("0'7'23", Ok(number(467, IntLitKind::Int, None)));
    assert_number_eq!("0xF'A1'b0", Ok(number(1024432, IntLitKind::Int, None)));
    assert_number_eq!("0b1'0'1'00", Ok(number(20, IntLitKind::Int, None)));
    assert_number_eq!("0'x0", Err(ParseNumberError::ExpectedDigitAfterQuote(1)));
    assert_number_eq!("123'u", Err(ParseNumberError::ExpectedDigitAfterQuote(3)));
    assert_number_eq!("0x'0", Err(ParseNumberError::ExpectedDigitBeforeQuote(2)));
    assert_number_eq!("0b'0", Err(ParseNumberError::ExpectedDigitBeforeQuote(2)));

    // suffix
    assert_number_eq!("42u", Ok(number(42, IntLitKind::UInt, None)));
    assert_number_eq!("42U", Ok(number(42, IntLitKind::UInt, None)));

    assert_number_eq!("42l", Ok(number(42, IntLitKind::Long, None)));
    assert_number_eq!("42L", Ok(number(42, IntLitKind::Long, None)));

    assert_number_eq!("42ul", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42uL", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42Ul", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42UL", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42lu", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42Lu", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42lU", Ok(number(42, IntLitKind::ULong, None)));
    assert_number_eq!("42LU", Ok(number(42, IntLitKind::ULong, None)));

    assert_number_eq!("42ll", Ok(number(42, IntLitKind::LongLong, None)));
    assert_number_eq!("42LL", Ok(number(42, IntLitKind::LongLong, None)));

    assert_number_eq!("42ull", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42uLL", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42Ull", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42ULL", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42llu", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42LLu", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42llU", Ok(number(42, IntLitKind::ULongLong, None)));
    assert_number_eq!("42LLU", Ok(number(42, IntLitKind::ULongLong, None)));

    assert_number_eq!("42z", Ok(number(42, IntLitKind::SSize, None)));
    assert_number_eq!("42Z", Ok(number(42, IntLitKind::SSize, None)));

    assert_number_eq!("42uz", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42uZ", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42Uz", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42UZ", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42zu", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42Zu", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42zU", Ok(number(42, IntLitKind::Size, None)));
    assert_number_eq!("42ZU", Ok(number(42, IntLitKind::Size, None)));

    // user-defined suffix
    assert_number_eq!("128abc", Ok(number(128, IntLitKind::Int, Some(3))));
    assert_number_eq!("128_abc_def", Ok(number(128, IntLitKind::Int, Some(3))));
    assert_number_eq!("128uf", Ok(number(128, IntLitKind::Int, Some(3))));
    assert_number_eq!("128zl", Ok(number(128, IntLitKind::Int, Some(3))));
    assert_number_eq!("128p", Ok(number(128, IntLitKind::Int, Some(3))));
    // avec caractère non ascii
    assert_number_eq!("128𝐀à", Ok(number(128, IntLitKind::Int, Some(3))));
    assert_number_eq!("128_é𝐀", Ok(number(128, IntLitKind::Int, Some(3))));

    // invalid suffix
    assert_number_eq!("12_abc'def", Err(ParseNumberError::InvalidCharInSuffix(6)));
    assert_number_eq!("12_e+1", Err(ParseNumberError::InvalidCharInSuffix(4)));
    assert_number_eq!("12_e-1", Err(ParseNumberError::InvalidCharInSuffix(4)));
    // `·` == `U+00B7` ne peut pas démarrer un suffixe (XID_Continue mais pas XID_Start)
    assert_number_eq!("123·abc", Err(ParseNumberError::InvalidSuffixStart(3)));
    assert_number_eq!("123a·bc", Ok(number(123, IntLitKind::Int, Some(3))));

    // prefix & suffix
    assert_number_eq!("0b10l", Ok(number(2, IntLitKind::Long, None)));
    assert_number_eq!("0x10l", Ok(number(16, IntLitKind::Long, None)));
    assert_number_eq!("0b10ud", Ok(number(2, IntLitKind::Int, Some(4))));
    assert_number_eq!("0b0b", Ok(number(0, IntLitKind::Int, Some(3))));
    assert_number_eq!("0x0x", Ok(number(0, IntLitKind::Int, Some(3))));
    assert_number_eq!("0xax", Ok(number(10, IntLitKind::Int, Some(3))));

    // promotions [tab:parse.icon.type]
    // (long == long long dans ce compilateur donc ça peut jamais être promu en long long)
    // decimal
    // int -> long
    assert_number_eq!("5000000000", Ok(number(5_000_000_000, IntLitKind::Long, None)));
    // unsigned -> unsigned long
    assert_number_eq!("5000000000u", Ok(number(5_000_000_000, IntLitKind::ULong, None)));
    // non decimal
    // int -> unsigned
    assert_number_eq!("0xAAAAAAAA", Ok(number(2_863_311_530, IntLitKind::UInt, None)));
    assert_number_eq!("027777777777", Ok(number(3_221_225_471, IntLitKind::UInt, None)));
    assert_number_eq!("0b11000000000000000000000000000000", Ok(number(3_221_225_472, IntLitKind::UInt, None)));
    // int -> unsigned -> long
    assert_number_eq!("0xAAAAAAAAA", Ok(number(45_812_984_490, IntLitKind::Long, None)));
    assert_number_eq!("0277777777777", Ok(number(25_769_803_775, IntLitKind::Long, None)));
    assert_number_eq!("0b110000000000000000000000000000000", Ok(number(6_442_450_944, IntLitKind::Long, None)));
    // int -> unsigned -> long -> unsigned long
    assert_number_eq!("0x8AC7230489E80000", Ok(number(10_000_000_000_000_000_000, IntLitKind::ULong, None)));
    assert_number_eq!("01053071060221172000000", Ok(number(10_000_000_000_000_000_000, IntLitKind::ULong, None)));
    assert_number_eq!("0b1000101011000111001000110000010010001001111010000000000000000000", Ok(number(10_000_000_000_000_000_000, IntLitKind::ULong, None)));
    // long -> unsigned long
    assert_number_eq!("0x8AC7230489E80000l", Ok(number(10_000_000_000_000_000_000, IntLitKind::ULong, None)));
    assert_number_eq!("01053071060221172000000l", Ok(number(10_000_000_000_000_000_000, IntLitKind::ULong, None)));
    assert_number_eq!("0b1000101011000111001000110000010010001001111010000000000000000000l", Ok(number(10_000_000_000_000_000_000, IntLitKind::ULong, None)));

    // max valid int (2^63 - 1 for decimal, 2^64 - 1 for non decimal)
    assert_number_eq!("9223372036854775807", Ok(number(9223372036854775807, IntLitKind::Long, None)));
    assert_number_eq!("9223372036854775808", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("0xFFFFFFFFFFFFFFFF", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("0x10000000000000000", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("01777777777777777777777", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("02000000000000000000000", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("0b1111111111111111111111111111111111111111111111111111111111111111", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("0b10000000000000000000000000000000000000000000000000000000000000000", Err(ParseNumberError::IntValueTooLarge));
    // max valid unsigned (2^64 - 1)
    assert_number_eq!("18446744073709551615u", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("18446744073709551616u", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("0xFFFFFFFFFFFFFFFFu", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("0x10000000000000000u", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("01777777777777777777777u", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("02000000000000000000000u", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("0b1111111111111111111111111111111111111111111111111111111111111111u", Ok(number(18446744073709551615, IntLitKind::ULong, None)));
    assert_number_eq!("0b10000000000000000000000000000000000000000000000000000000000000000u", Err(ParseNumberError::IntValueTooLarge));

    // large numbers
    assert_number_eq!("9999999999999999999999999999999999999999999999999999999999999999999999999999", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("0b1111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("077777777777777777777777777777777777777777777777777777777777777777777777777", Err(ParseNumberError::IntValueTooLarge));
    assert_number_eq!("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", Err(ParseNumberError::IntValueTooLarge));

    // number is 0 (so '0' is not the octal prefix)
    assert_number_eq!("0", Ok(number(0, IntLitKind::Int, None)));
    assert_number_eq!("0u", Ok(number(0, IntLitKind::UInt, None)));
    assert_number_eq!("0_abc", Ok(number(0, IntLitKind::Int, Some(1))));

    // empty number
    assert_number_eq!("0x", Err(ParseNumberError::EmptyNumber { base: 16 }));
    assert_number_eq!("0xx", Err(ParseNumberError::EmptyNumber { base: 16 }));
    assert_number_eq!("0b", Err(ParseNumberError::EmptyNumber { base: 2 }));
    assert_number_eq!("0bb", Err(ParseNumberError::EmptyNumber { base: 2 }));

    // unexpected char
    assert_number_eq!("0x1e+1", Err(ParseNumberError::UnexpectedChar(4)));
    assert_number_eq!("0x1e-1", Err(ParseNumberError::UnexpectedChar(4)));

    // invalid digit
    assert_number_eq!("0b101310", Err(ParseNumberError::InvalidDigit { base: 2, pos: 5 }));
    assert_number_eq!("0b1'0'1'31'0", Err(ParseNumberError::InvalidDigit { base: 2, pos: 8 }));
    assert_number_eq!("012934", Err(ParseNumberError::InvalidDigit { base: 8, pos: 3 }));
    assert_number_eq!("0'12'93'4", Err(ParseNumberError::InvalidDigit { base: 8, pos: 5 }));
}

#[test]
fn float_literal() {
    fn number(value: f64, kind: FloatLitKind, ud_suffix: Option<u32>) -> NumberLit {
        NumberLit {
            kind: NumberLitKind::Float { kind, value },
            ud_suffix: ud_suffix.map(|pos| NonZeroU32::new(pos).unwrap()),
        }
    }

    // starting with 0
    assert_number_eq!("0.", Ok(number(0.0, FloatLitKind::Double, None)));
    assert_number_eq!("0.0", Ok(number(0.0, FloatLitKind::Double, None)));
    assert_number_eq!("0e3", Ok(number(0.0, FloatLitKind::Double, None)));
    assert_number_eq!("0128e3", Ok(number(128_000.0, FloatLitKind::Double, None)));
    assert_number_eq!("0128.0", Ok(number(128.0, FloatLitKind::Double, None)));
    assert_number_eq!("0128p", Err(ParseNumberError::InvalidDigit { pos: 3, base: 8 }));

    // suffix
    assert_number_eq!("42.0f", Ok(number(42.0, FloatLitKind::Float, None)));
    assert_number_eq!("42.0F", Ok(number(42.0, FloatLitKind::Float, None)));
    assert_number_eq!("0x42.0p0f", Ok(number(66.0, FloatLitKind::Float, None)));
    assert_number_eq!("0x42.0p0F", Ok(number(66.0, FloatLitKind::Float, None)));

    assert_number_eq!("42.0l", Ok(number(42.0, FloatLitKind::LongDouble, None)));
    assert_number_eq!("42.0L", Ok(number(42.0, FloatLitKind::LongDouble, None)));
    assert_number_eq!("0x42.0p0l", Ok(number(66.0, FloatLitKind::LongDouble, None)));
    assert_number_eq!("0x42.0p0L", Ok(number(66.0, FloatLitKind::LongDouble, None)));

    assert_number_eq!("42.0f16", Ok(number(42.0, FloatLitKind::F16, None)));
    assert_number_eq!("42.0F16", Ok(number(42.0, FloatLitKind::F16, None)));
    assert_number_eq!("0x42.0p0f16", Ok(number(66.0, FloatLitKind::F16, None)));
    assert_number_eq!("0x42.0p0F16", Ok(number(66.0, FloatLitKind::F16, None)));

    assert_number_eq!("42.0f32", Ok(number(42.0, FloatLitKind::F32, None)));
    assert_number_eq!("42.0F32", Ok(number(42.0, FloatLitKind::F32, None)));
    assert_number_eq!("0x42.0p0f32", Ok(number(66.0, FloatLitKind::F32, None)));
    assert_number_eq!("0x42.0p0F32", Ok(number(66.0, FloatLitKind::F32, None)));

    assert_number_eq!("42.0f64", Ok(number(42.0, FloatLitKind::F64, None)));
    assert_number_eq!("42.0F64", Ok(number(42.0, FloatLitKind::F64, None)));
    assert_number_eq!("0x42.0p0f64", Ok(number(66.0, FloatLitKind::F64, None)));
    assert_number_eq!("0x42.0p0F64", Ok(number(66.0, FloatLitKind::F64, None)));

    assert_number_eq!("42.0f128", Ok(number(42.0, FloatLitKind::F128, None)));
    assert_number_eq!("42.0F128", Ok(number(42.0, FloatLitKind::F128, None)));
    assert_number_eq!("0x42.0p0f128", Ok(number(66.0, FloatLitKind::F128, None)));
    assert_number_eq!("0x42.0p0F128", Ok(number(66.0, FloatLitKind::F128, None)));

    assert_number_eq!("42.0bf16", Ok(number(42.0, FloatLitKind::BF16, None)));
    assert_number_eq!("42.0BF16", Ok(number(42.0, FloatLitKind::BF16, None)));
    assert_number_eq!("0x42.0p0bf16", Ok(number(66.0, FloatLitKind::BF16, None)));
    assert_number_eq!("0x42.0p0BF16", Ok(number(66.0, FloatLitKind::BF16, None)));

    // user-defined suffix
    assert_number_eq!("42.0u", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    assert_number_eq!("42.0fu", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    assert_number_eq!("42.0_abc", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    assert_number_eq!("42.0lol", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    assert_number_eq!("42.0bF16", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    assert_number_eq!("0x42.0p0bF16", Ok(number(66.0, FloatLitKind::Double, Some(8))));
    // avec caractère non ascii
    assert_number_eq!("42.0é𝐀", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    assert_number_eq!("42.0𝐀é", Ok(number(42.0, FloatLitKind::Double, Some(4))));
    // `·` == `U+00B7` ne peut pas démarrer un suffixe (XID_Continue mais pas XID_Start)
    assert_number_eq!("42.0·abc", Err(ParseNumberError::InvalidSuffixStart(4)));
    assert_number_eq!("42.0a·bc", Ok(number(42.0, FloatLitKind::Double, Some(4))));

    // exponent
    assert_number_eq!("1.0e3", Ok(number(1000.0, FloatLitKind::Double, None)));
    assert_number_eq!("1.0E3", Ok(number(1000.0, FloatLitKind::Double, None)));
    assert_number_eq!("1.0e+3", Ok(number(1000.0, FloatLitKind::Double, None)));
    assert_number_eq!("1.0e-3", Ok(number(0.001, FloatLitKind::Double, None)));
    assert_number_eq!("1e3", Ok(number(1000.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x1p3", Ok(number(8.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x1P3", Ok(number(8.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x1p+3", Ok(number(8.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x1p-3", Ok(number(0.125, FloatLitKind::Double, None)));
    assert_number_eq!("0xep3", Ok(number(112.0, FloatLitKind::Double, None)));
    assert_number_eq!("3ee", Err(ParseNumberError::ExpectedExponentValue(2)));
    assert_number_eq!("3ee", Err(ParseNumberError::ExpectedExponentValue(2)));
    assert_number_eq!("3e+e", Err(ParseNumberError::ExpectedExponentValue(3)));
    assert_number_eq!("3e-e", Err(ParseNumberError::ExpectedExponentValue(3)));
    assert_number_eq!("0x1p", Err(ParseNumberError::ExpectedExponentValue(3)));
    assert_number_eq!("0x1pe", Err(ParseNumberError::ExpectedExponentValue(4)));
    assert_number_eq!("0x1p+e", Err(ParseNumberError::ExpectedExponentValue(5)));
    assert_number_eq!("0x1p-e", Err(ParseNumberError::ExpectedExponentValue(5)));
    assert_number_eq!("0x10.0", Err(ParseNumberError::NoExponentInHexFloat));
    assert_number_eq!("0x10.0e3", Err(ParseNumberError::NoExponentInHexFloat));

    // leading 0 in exponent
    assert_number_eq!("3e01", Ok(number(30.0, FloatLitKind::Double, None)));
    assert_number_eq!("3e+01", Ok(number(30.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x3p01", Ok(number(6.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x3p+01", Ok(number(6.0, FloatLitKind::Double, None)));

    // exponent & suffix
    assert_number_eq!("1e5f", Ok(number(100_000.0, FloatLitKind::Float, None)));
    assert_number_eq!("1.5e5l", Ok(number(150_000.0, FloatLitKind::LongDouble, None)));
    assert_number_eq!("1e5el", Ok(number(100_000.0, FloatLitKind::Double, Some(3))));
    assert_number_eq!("0x1p5f", Ok(number(32.0, FloatLitKind::Float, None)));
    assert_number_eq!("0x1p5p", Ok(number(32.0, FloatLitKind::Double, Some(5))));
    assert_number_eq!("0x1p5e3", Ok(number(32.0, FloatLitKind::Double, Some(5))));
    assert_number_eq!("0x1p5a3", Ok(number(32.0, FloatLitKind::Double, Some(5))));

    // dot in exponent
    assert_number_eq!("3e5.4", Err(ParseNumberError::DotInExponent));
    assert_number_eq!("3.2e5.4", Err(ParseNumberError::DotInExponent));
    assert_number_eq!("0x3p5.4", Err(ParseNumberError::DotInExponent));

    // no leading/trailing 0
    assert_number_eq!(".5", Ok(number(0.5, FloatLitKind::Double, None)));
    assert_number_eq!(".5f", Ok(number(0.5, FloatLitKind::Float, None)));
    assert_number_eq!(".5e3", Ok(number(500.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x.5p3", Ok(number(2.5, FloatLitKind::Double, None)));
    assert_number_eq!("42.", Ok(number(42.0, FloatLitKind::Double, None)));
    assert_number_eq!("42.f", Ok(number(42.0, FloatLitKind::Float, None)));
    assert_number_eq!("42.e3", Ok(number(42_000.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x42.p3", Ok(number(528.0, FloatLitKind::Double, None)));

    // hex float
    assert_number_eq!("0x1p3", Ok(number(8.0, FloatLitKind::Double, None)));
    assert_number_eq!("0X1p3", Ok(number(8.0, FloatLitKind::Double, None)));
    assert_number_eq!("0xaBcDeFp2", Ok(number(45037500.0, FloatLitKind::Double, None)));
    assert_number_eq!("0xAbCdEfp2", Ok(number(45037500.0, FloatLitKind::Double, None)));

    // empty hex mantissa
    assert_number_eq!("0xp3", Err(ParseNumberError::EmptyHexMantissa));
    assert_number_eq!("0x.p3", Err(ParseNumberError::EmptyHexMantissa));

    // too many dots
    assert_number_eq!("12.3.6", Err(ParseNumberError::TooManyDots));
    assert_number_eq!("0x12.3.6p3", Err(ParseNumberError::TooManyDots));

    // binary float
    assert_number_eq!("0b1101.0", Err(ParseNumberError::BinaryFloat));
    assert_number_eq!("0b1101e3", Err(ParseNumberError::BinaryFloat));

    // with quotes
    assert_number_eq!("12'3.5'5e1'0", Ok(number(1235500000000.0, FloatLitKind::Double, None)));
    assert_number_eq!("0x10'f.0'0p1'0", Ok(number(277504.0, FloatLitKind::Double, None)));

    assert_number_eq!("1.'5e3", Err(ParseNumberError::ExpectedDigitBeforeQuote(2)));
    assert_number_eq!("1.5'e3", Err(ParseNumberError::ExpectedDigitAfterQuote(3)));
    assert_number_eq!("1.5e'3", Err(ParseNumberError::ExpectedExponentValue(4)));
    assert_number_eq!("1.5e+'3", Err(ParseNumberError::ExpectedExponentValue(5)));

    assert_number_eq!("0x'1.5p3", Err(ParseNumberError::ExpectedDigitBeforeQuote(2)));
    assert_number_eq!("0x1.'5p3", Err(ParseNumberError::ExpectedDigitBeforeQuote(4)));
    assert_number_eq!("0x1.5'p3", Err(ParseNumberError::ExpectedDigitAfterQuote(5)));
    assert_number_eq!("0x1.5p'3", Err(ParseNumberError::ExpectedExponentValue(6)));
    assert_number_eq!("0x1.5p+'3", Err(ParseNumberError::ExpectedExponentValue(7)));
}
