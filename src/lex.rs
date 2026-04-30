//! ~ lexical analysis ~

use crate::name::Name;
use arrayvec::ArrayVec;
use core::{num::IntErrorKind, str::Bytes};
use std::{
    borrow::Cow,
    bstr::ByteString,
    debug_assert_matches,
    num::{NonZeroU32, ParseFloatError, ParseIntError},
    ops::Range,
    str::Chars,
};

pub fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{B}' | '\u{C}')
}

pub fn is_ident_start(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '_') || unicode_ident::is_xid_start(c)
}

pub fn is_ident_continue(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '_' | '0'..='9') || unicode_ident::is_xid_continue(c)
}

pub fn is_basic_charset(c: char) -> bool {
    matches!(c,
        '\t' | '\u{B}' | '\u{C}' | ' ' | '\n' | '!' | '"' | '#' | '$' | '%' | '&' |
        '\'' | '(' | ')' | '*' | '+' | ',' | '-' | '.' | '/' | ':' | ';' | '<' | '=' |
        '>' | '?' | '@' | '0'..='9' | 'a'..='z' | 'A'..='Z' | '[' | '\\' | ']' | '^' |
        '_' | '`' | '{' | '|' | '}' | '~'
    )
}

fn encode_utf8(c: char, dst: &mut impl Extend<u8>) {
    let mut bytes = [0; 4];
    c.encode_utf8(&mut bytes);
    dst.extend(bytes.into_iter().take(c.len_utf8()));
}

fn encode_utf16(c: char, dst: &mut impl Extend<u8>) {
    let mut b = [0; 2];
    c.encode_utf16(&mut b);
    dst.extend(
        b.into_iter()
            .take(c.len_utf16())
            .flat_map(|u| u.to_le_bytes()),
    );
}

fn encode_utf32(c: char, dst: &mut impl Extend<u8>) {
    dst.extend((c as u32).to_le_bytes());
}

/// récupère le lexème sous sa forme "traitée" (donc sans line continuations ni UCNs)
/// `src_lexeme` correspond à la partie du texte brut dont on veut récupérer le
/// lexème
///
/// pour les tokens de type `TokenKind::Name(name)`, on peut le récupérer directement
/// avec `name.as_str()` donc pas besoin d'utiliser cette fonction dans ce cas
pub fn extract_lexeme<'a>(kind: &TokenKind, src_lexeme: &'a str) -> Cow<'a, str> {
    match kind {
        TokenKind::Str(StrKind::Raw, ..) => extract_lexeme_raw_str(src_lexeme),
        _ => extract_lexeme_basic(src_lexeme),
    }
}

fn extract_lexeme_basic(src_lexeme: &str) -> Cow<'_, str> {
    if src_lexeme.contains('\\') {
        Cow::Owned(remove_line_conts_and_decode_ucns(src_lexeme))
    } else {
        Cow::Borrowed(src_lexeme)
    }
}

fn extract_lexeme_raw_str(raw_str: &str) -> Cow<'_, str> {
    // il peut y avoir des line continuations ou UCN dans le préfixe et/ou suffixe
    // qu'il faut enlever mais on touche pas à la chaîne en elle-même (car c'est
    // une raw string)
    let str_start = raw_str.find('"').expect("strings should start with a `\"`");
    let str_end = raw_str.rfind('"').expect("strings should end with a `\"`") + 1;
    let prefix = &raw_str[..str_start];
    let suffix = &raw_str[str_end..];

    if prefix.contains('\\') || suffix.contains('\\') {
        let prefix = if prefix.contains('\\') {
            &remove_line_conts_and_decode_ucns(prefix)
        } else {
            prefix
        };
        let suffix = if suffix.contains('\\') {
            &remove_line_conts_and_decode_ucns(suffix)
        } else {
            suffix
        };

        Cow::Owned([prefix, &raw_str[str_start..str_end], suffix].join(""))
    } else {
        Cow::Borrowed(raw_str)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum TokenKind {
    // preprocessor
    Hash,     // #
    HashHash, // ##

    // punctuation
    BraceL,     // (
    BraceR,     // )
    BracketL,   // [
    BracketR,   // ]
    ParenL,     // (
    ParenR,     // )
    SpliceL,    // [:
    SpliceR,    // :]
    Semi,       // ;
    Colon,      // :
    DotDotDot,  // ...
    Question,   // ?
    ColonColon, // ::
    Dot,        // .
    DotStar,    // .*
    Arrow,      // ->
    ArrowStar,  // ->*
    Tilde,      // ~
    Bang,       // !
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Percent,    // %
    Caret,      // ^
    CatEars,    // ^^
    And,        // &
    Or,         // |
    Eq,         // =
    PlusEq,     // +=
    MinusEq,    // -=
    StarEq,     // *=
    SlashEq,    // /=
    PercentEq,  // %=
    CaretEq,    // ^=
    AndEq,      // &=
    OrEq,       // |=
    EqEq,       // ==
    Ne,         // !=
    Lt,         // <
    Gt,         // >
    LtEq,       // <=
    GtEq,       // >=
    Spaceship,  // <=>
    AndAnd,     // &&
    OrOr,       // ||
    LtLt,       // <<
    GtGt,       // >>
    LtLtEq,     // <<=
    GtGtEq,     // >>=
    PlusPlus,   // ++
    MinusMinus, // --
    Comma,      // ,

    Name(Name),
    Number,
    Char(Encoding, u32, Option<UdSuffix>),
    Multichar(u32, Option<UdSuffix>),
    // todo: le ByteString ça serait bien de l'interner...
    // ou alors ne pas le stocker ici et extraire la valeur plus tard ?
    Str(StrKind, Encoding, ByteString, Option<UdSuffix>),

    Eof,
    Unknown,
}

const MAX_MULTICHAR_LEN: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StrKind {
    NonRaw,
    Raw,
}

/// représente la position du début du suffixe user-defined (offset dans le lexème)
pub type UdSuffix = NonZeroU32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Ordinary,
    Wide,
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    fn from_prefix(prefix: &str) -> Self {
        match prefix {
            "" => Self::Ordinary,
            "L" => Self::Wide,
            "u8" => Self::Utf8,
            "u" => Self::Utf16,
            "U" => Self::Utf32,
            _ => panic!("`{prefix}` is not a valid encoding prefix"),
        }
    }
}

fn is_char_prefix(s: &str) -> bool {
    matches!(s, "u8" | "u" | "U" | "L")
}

fn is_str_prefix(s: &str) -> bool {
    is_char_prefix(s) || matches!(s, "R" | "u8R" | "uR" | "UR" | "LR")
}

fn to_alt_token(s: &str) -> Option<TokenKind> {
    use TokenKind::*;

    match s {
        "and" => Some(AndAnd),
        "and_eq" => Some(AndEq),
        "bitand" => Some(And),
        "bitor" => Some(Or),
        "compl" => Some(Tilde),
        "not" => Some(Bang),
        "not_eq" => Some(Ne),
        "or" => Some(OrOr),
        "or_eq" => Some(OrEq),
        "xor" => Some(Caret),
        "xor_eq" => Some(CaretEq),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntLitKind {
    Int,
    Long,
    LongLong,
    UInt,
    ULong,
    ULongLong,
    SSize,
    Size,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatLitKind {
    Float,
    Double,
    LongDouble,
    F16,
    F32,
    F64,
    F128,
    BF16,
}

#[derive(Clone, PartialEq, Debug)]
pub enum NumberLitKind {
    Int {
        kind: IntLitKind,
        value: i128, // todo: BigInt
    },
    Float {
        kind: FloatLitKind,
        value: f64, // todo: BigFloat
    },
}

#[derive(Clone, PartialEq, Debug)]
pub struct NumberLit {
    pub kind: NumberLitKind,
    pub ud_suffix: Option<UdSuffix>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ParseNumberError {
    InvalidDigit { pos: usize, base: u32 },
    UnexpectedChar(usize),
    InvalidSuffixStart(usize),
    InvalidCharInSuffix(usize),
    EmptyNumber { base: u32 },
    ExpectedDigitBeforeQuote(usize),
    ExpectedDigitAfterQuote(usize),
    IntValueTooLarge,
    ExpectedExponentValue(usize),
    NoExponentInHexFloat,
    EmptyHexMantissa,
    DotInExponent,
    TooManyDots,
    BinaryFloat,
    Other,
}

impl From<ParseIntError> for ParseNumberError {
    fn from(error: ParseIntError) -> Self {
        match error.kind() {
            IntErrorKind::PosOverflow => ParseNumberError::IntValueTooLarge,
            _ => ParseNumberError::Other,
        }
    }
}

impl From<ParseFloatError> for ParseNumberError {
    fn from(_: ParseFloatError) -> Self {
        ParseNumberError::Other
    }
}

/// parse le nombre pour créer un littéral entier ou flottant.
///
/// le nombre doit être le lexème (donc sans line continuations etc) d'un token
/// Number (qui est un preprocessing number)
#[allow(clippy::get_first)]
pub fn parse_number(number: &str) -> Result<NumberLit, ParseNumberError> {
    use ParseNumberError::*;
    debug_assert!(!number.is_empty());
    debug_assert!(number.bytes().last() != Some(b'\''));

    // on itère sur les bytes et pas sur les chars pour éviter de devoir décoder
    // les chars et car 99.99% des nombres sont en ascii
    // la seule subtilité est de détecter correctement le début du suffixe qui
    // peut être n'importe quel caractère id_start (voir commentaire plus bas)
    let bytes = number.as_bytes();
    let (base, number_start) = match (bytes.get(0), bytes.get(1)) {
        (Some(b'0'), Some(b'b' | b'B')) => (2, 2),
        (Some(b'0'), Some(b'x' | b'X')) => (16, 2),
        (Some(b'0'), c) => {
            if c.is_none() || c.is_some_and(|&c| c == b'.' || is_ident_start(c as char)) {
                // le '0' fait en fait partie du nombre, ce n'est pas un préfixe
                (8, 0)
            } else {
                (8, 1)
            }
        }
        _ => (10, 0),
    };

    let is_digit = |c: u8| {
        if base == 16 {
            c.is_ascii_hexdigit()
        } else {
            c.is_ascii_digit()
        }
    };

    let mut has_quotes = false;
    let mut dot_pos = None;
    let mut exp_start = None;
    let mut suffix_start = None;
    let mut it = number[number_start..].bytes();
    let pos = |it: &Bytes| number.len() - it.len() - 1;

    while let Some(c) = it.next() {
        match c {
            b'\'' => {
                has_quotes = true;
                let i = pos(&it);

                if !bytes.get(i.saturating_sub(1)).is_some_and(|&c| is_digit(c)) {
                    return Err(ExpectedDigitBeforeQuote(i));
                }
                if !bytes.get(i + 1).is_some_and(|&c| is_digit(c)) {
                    return Err(ExpectedDigitAfterQuote(i));
                }
            }

            b'.' => {
                if dot_pos.is_some() {
                    return Err(TooManyDots);
                }
                dot_pos = Some(pos(&it));
            }

            // on regarde uniquement le premier byte pour savoir si le caractère
            // est id_start, ça ne fonctionne pas dans le cas général (par ex
            // 𝐀 == U+1D400 (0xF0 0x9D 0x90 0x80) est bien id_start mais
            // 😀 == U+1F600 (0xF0 0x9F 0x98 0x80) ne l'est pas, pourtant le
            // premier byte est le même)
            // ça fonctionne quand même ici car :
            //   - le lexer (qui produit des pp numbers) vérifie que le char
            //     en lui-même est id_continue (sinon il ne ferait pas partie du nombre)
            //   - on suppose qu'on nous a passé un pp number valide
            //   - pour chaque byte `b` (de 0x00 à 0xFF), on a
            //     is_ident_start(char::from_u32(b)) == is_ident_start(b as char)
            //
            // autrement dit, si le caractère était id_continue dans le pp number,
            // le test is_ident_start ici va passer, et si il n'était pas id_continue
            // alors il ne fait pas partie du nombre donc on le verra même pas
            // ici
            //
            // par contre, il se peut que que le caractère en question était
            // id_continue mais pas id_start, ce test va pourtant quand même le
            // considérer comme id_start et donc comme début du suffixe mais c'est
            // pas grave puisque par la suite on vérifie que le premier char
            // (pas seulement le premier byte) du suffixe est bien id_start,
            // sinon on retourne une erreur
            c if is_ident_start(c as char) => {
                if exp_start.is_none()
                    && (matches!(c, b'e' | b'E') && base != 16
                        || matches!(c, b'p' | b'P') && base == 16)
                {
                    exp_start = Some(pos(&it));
                    match it.next() {
                        Some(b'+' | b'-') => {
                            if !it.next().is_some_and(|c| c.is_ascii_digit()) {
                                return Err(ExpectedExponentValue(pos(&it)));
                            }
                        }
                        Some(c) if c.is_ascii_digit() => {}
                        _ => return Err(ExpectedExponentValue(pos(&it))),
                    }

                    loop {
                        match it.clone().next() {
                            Some(b'0'..=b'9' | b'\'') => {}
                            Some(b'.') => return Err(DotInExponent),
                            _ => break,
                        }
                        it.next();
                    }
                } else if exp_start.is_some() || !is_digit(c) {
                    suffix_start = Some(pos(&it));
                    break;
                }
            }

            c => {
                if !c.is_ascii_digit() {
                    return Err(UnexpectedChar(pos(&it)));
                }
            }
        }
    }

    let number_end = suffix_start.unwrap_or(number.len());
    let suffix = &number[number_end..];

    if !suffix.is_empty() {
        if !suffix.starts_with(is_ident_start) {
            return Err(InvalidSuffixStart(number_end));
        }
        if let Some(i) = suffix.find(|c| !is_ident_continue(c)) {
            return Err(InvalidCharInSuffix(number_end + i));
        }
    }

    let raw_digits = &number[number_start..number_end];
    if raw_digits.is_empty() {
        return Err(EmptyNumber { base });
    }
    let digits = remove_quotes(raw_digits, has_quotes);

    #[rustfmt::skip]
    let is_float = dot_pos.is_some()
        || exp_start.is_some()
        || matches!(suffix, "f" | "f16" | "f32" | "f64" | "f128" | "bf16" | "F" | "F16" | "F32" | "F64" | "F128" | "BF16");

    if is_float {
        if base == 2 {
            return Err(BinaryFloat);
        }

        let value = if base == 16 {
            // todo: mieux implémenter les hex floats
            let overflow = |e: &ParseIntError| {
                if *e.kind() == IntErrorKind::PosOverflow {
                    todo!("mantissa of arbitrary size in hexadecimal floating-point literals");
                }
            };
            let exp_start = exp_start.ok_or(NoExponentInHexFloat)?;

            // peut-être que has_quotes est vrai sans pour autant qu'il y ait des quotes
            // dans _cette partie là_ du nombre, auquel cas on aura enlevé les quotes
            // pour rien mais c'est pas grave (sinon il faudrait que remove_quotes
            // parcourt une fois de plus la string pour savoir si il y a des quotes)
            let before_dot_str = remove_quotes(
                &number[number_start..dot_pos.unwrap_or(exp_start)],
                has_quotes,
            );

            let before_dot = if before_dot_str.is_empty() {
                if dot_pos.is_none() {
                    return Err(EmptyHexMantissa);
                }
                0.0
            } else {
                u128::from_str_radix(&before_dot_str, 16).inspect_err(overflow)? as f64
            };

            let mantissa = if let Some(dot_pos) = dot_pos {
                let after_dot_str = remove_quotes(&number[dot_pos + 1..exp_start], has_quotes);
                let after_dot = if after_dot_str.is_empty() {
                    if before_dot_str.is_empty() {
                        return Err(EmptyHexMantissa);
                    }
                    0.0
                } else {
                    u128::from_str_radix(&after_dot_str, 16).inspect_err(overflow)? as f64
                };
                before_dot + after_dot / 16.0f64.powi(after_dot_str.len() as i32)
            } else {
                before_dot
            };

            let exp_str = remove_quotes(&number[exp_start + 1..number_end], has_quotes);
            let exp = exp_str.parse::<f64>()?;
            mantissa * 2.0f64.powf(exp)
        } else {
            digits.parse()?
        };

        Ok(make_float_number_lit(value, suffix, suffix_start))
    } else {
        // on ne peut pas vérifier les digits avant car il faut savoir si c'est un
        // flottant avant de parler (par ex on pourrait croire que "0189e3" est invalide
        // car on dirait que c'est un nombre octal (qui contient des digits "89" invalides)
        // sauf qu'en fait c'est un flottant car il finit par "e3")
        if matches!(base, 2 | 8) {
            // on itère sur le nombre raw (qui contient les quotes) pour avoir
            // les bons indices pour le message d'erreur
            for (i, c) in raw_digits.as_bytes().iter().enumerate() {
                if base == 2 && !matches!(c, b'0' | b'1' | b'\'')
                    || base == 8 && !matches!(c, b'0'..=b'7' | b'\'')
                {
                    return Err(InvalidDigit {
                        pos: number_start + i,
                        base,
                    });
                }
            }
        }

        let value = i128::from_str_radix(&digits, base).or_else(|e| match e.kind() {
            IntErrorKind::Zero => Ok(0),
            _ => Err(e),
        })?;

        make_int_number_lit(value, base, suffix, suffix_start).ok_or(IntValueTooLarge)
    }
}

fn remove_quotes(src: &str, has_quotes: bool) -> Cow<'_, str> {
    if has_quotes {
        Cow::Owned(src.replace("'", ""))
    } else {
        Cow::Borrowed(src)
    }
}

fn make_int_number_lit(
    value: i128,
    base: u32,
    suffix: &str,
    suffix_start: Option<usize>,
) -> Option<NumberLit> {
    type Int = i32;
    type Long = i64;
    type LongLong = i64;
    type UInt = u32;
    type ULong = u64;
    type ULongLong = u64;
    type SSize = isize;
    type Size = usize;

    let mut ud_suffix = None;
    let kind = match suffix {
        "u" | "U" => {
            if UInt::try_from(value).is_ok() {
                IntLitKind::UInt
            } else if ULong::try_from(value).is_ok() {
                IntLitKind::ULong
            } else if ULongLong::try_from(value).is_ok() {
                IntLitKind::ULongLong
            } else {
                return None;
            }
        }

        "l" | "L" => {
            if Long::try_from(value).is_ok() {
                IntLitKind::Long
            } else if base != 10 && ULong::try_from(value).is_ok() {
                IntLitKind::ULong
            } else if LongLong::try_from(value).is_ok() {
                IntLitKind::LongLong
            } else if base != 10 && ULongLong::try_from(value).is_ok() {
                IntLitKind::ULongLong
            } else {
                return None;
            }
        }

        "ul" | "uL" | "Ul" | "UL" | "lu" | "Lu" | "lU" | "LU" => {
            if ULong::try_from(value).is_ok() {
                IntLitKind::ULong
            } else if ULongLong::try_from(value).is_ok() {
                IntLitKind::ULongLong
            } else {
                return None;
            }
        }

        "ll" | "LL" => {
            if LongLong::try_from(value).is_ok() {
                IntLitKind::LongLong
            } else if base != 10 && ULongLong::try_from(value).is_ok() {
                IntLitKind::ULongLong
            } else {
                return None;
            }
        }

        "ull" | "uLL" | "Ull" | "ULL" | "llu" | "LLu" | "llU" | "LLU" => {
            if ULongLong::try_from(value).is_ok() {
                IntLitKind::ULongLong
            } else {
                return None;
            }
        }

        "z" | "Z" => {
            if SSize::try_from(value).is_ok() {
                IntLitKind::SSize
            } else if base != 10 && Size::try_from(value).is_ok() {
                IntLitKind::Size
            } else {
                return None;
            }
        }

        "uz" | "uZ" | "Uz" | "UZ" | "zu" | "Zu" | "zU" | "ZU" => {
            if Size::try_from(value).is_ok() {
                IntLitKind::Size
            } else {
                return None;
            }
        }

        _ => {
            ud_suffix = suffix_start.map(|pos| NonZeroU32::new(pos as u32).unwrap());

            if Int::try_from(value).is_ok() {
                IntLitKind::Int
            } else if base != 10 && UInt::try_from(value).is_ok() {
                IntLitKind::UInt
            } else if Long::try_from(value).is_ok() {
                IntLitKind::Long
            } else if base != 10 && ULong::try_from(value).is_ok() {
                IntLitKind::ULong
            } else if LongLong::try_from(value).is_ok() {
                IntLitKind::LongLong
            } else if base != 10 && ULongLong::try_from(value).is_ok() {
                IntLitKind::ULongLong
            } else {
                return None;
            }
        }
    };

    Some(NumberLit {
        kind: NumberLitKind::Int { kind, value },
        ud_suffix,
    })
}

fn make_float_number_lit(value: f64, suffix: &str, suffix_start: Option<usize>) -> NumberLit {
    let mut ud_suffix = None;
    // todo: range check?
    let kind = match suffix {
        "f" | "F" => FloatLitKind::Float,
        "l" | "L" => FloatLitKind::LongDouble,
        "f16" | "F16" => FloatLitKind::F16,
        "f32" | "F32" => FloatLitKind::F32,
        "f64" | "F64" => FloatLitKind::F64,
        "f128" | "F128" => FloatLitKind::F128,
        "bf16" | "BF16" => FloatLitKind::BF16,
        _ => {
            ud_suffix = suffix_start.map(|pos| NonZeroU32::new(pos as u32).unwrap());
            FloatLitKind::Double
        }
    };

    NumberLit {
        kind: NumberLitKind::Float { kind, value },
        ud_suffix,
    }
}

// todo: ça serait peut-être mieux de "pré-lexer" le texte pour enlever les
// line continuations et décoder les UCNs comme ça on pourrait lexer le texte
// sans se préoccuper de tout ça (ça fait quand même pas mal de travail pour chaque
// caractère, y compris quand on peek)
// ça voudrait dire que le lexer ne verrait plus le texte original donc les locations
// seront incorrectes mais on pourrait les remapper
#[derive(Clone)]
struct SkipLineCont<'a> {
    raw: Chars<'a>,
}

impl SkipLineCont<'_> {
    fn len(&self) -> usize {
        self.raw.as_str().len()
    }
}

impl Iterator for SkipLineCont<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        eat_line_cont(&mut self.raw);
        self.raw.next()
    }
}

fn eat_newline(chars: &mut (impl Iterator<Item = char> + Clone)) -> bool {
    let mut it = chars.clone();
    match it.next() {
        Some('\r') if let Some('\n') = it.clone().next() => {
            it.next();
        }
        Some('\r') | Some('\n') => {}
        _ => return false,
    }

    *chars = it;
    true
}

fn eat_line_cont(chars: &mut Chars) -> bool {
    let mut it = chars.clone();
    let mut ate_line_cont = false;

    while let Some('\\') = it.next() {
        // c'est une line continuation si on finit par tomber sur un newline
        // (éventuellement précédé d'espaces)
        let is_line_cont = loop {
            if eat_newline(&mut it) {
                break true;
            }

            if !it.next().is_some_and(is_whitespace) {
                break false;
            }
        };

        if is_line_cont {
            ate_line_cont = true;
            *chars = it.clone();
        } else {
            return ate_line_cont;
        }
    }

    ate_line_cont
}

fn remove_line_conts_and_decode_ucns(src: &str) -> String {
    debug_assert!(
        src.contains('\\'),
        "why are you calling this function if there is nothing to do??"
    );

    let mut res = String::new();
    let mut it = SkipLineCont { raw: src.chars() };

    loop {
        let next = if let Some(Ok(c)) = eat_ucn(&mut it) {
            c
        } else if let Some(c) = it.next() {
            c
        } else {
            break;
        };

        res.push(next);
    }

    res
}

#[derive(Clone, PartialEq, Debug)]
pub enum LexError {
    Char(CharError, Range<u32>),
    Str(StrError, Range<u32>),
    Unterminated(UnterminatedKind, u32),
    Escape(EscapeError, Range<u32>),
    NumericEscapeOutOfRange(Range<u32>),
    UnexpectedBasicUcn {
        c: char,
        is_control: bool,
        span: Range<u32>,
    },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UnterminatedKind {
    MultilineComment,
    Char,
    Str,
    RawStr { delim: Option<String> },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CharError {
    Empty,
    Unmappable,
    TooManyChars,
    MulticharPrefix,
    NonAsciiInMultichar,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StrError {
    InvalidCharInDelim,
    TooManyCharsInDelim,
}

#[derive(Clone, PartialEq, Debug)]
pub enum EscapeError {
    UnknownEscape,
    ExpectedDigits { n: u32, base: u32 },
    ExpectedOpenBrace,
    ExpectedOpenBraceOrHexDigit,
    NoCloseBrace,
    InvalidDigitInBraces { base: u32 },
    EmptyBraces,
    InvalidUcnValue,
    InvalidUcnName,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EscapeSeqKind {
    Simple,
    Numeric,
}

fn eat_escape_seq(chars: &mut SkipLineCont) -> Option<Result<(u128, EscapeSeqKind), EscapeError>> {
    use EscapeSeqKind::*;

    let mut it = chars.clone();
    if it.next() == Some('\\') {
        let res = match it.next() {
            Some('n') => Ok(('\n' as u128, Simple)),
            Some('r') => Ok(('\r' as u128, Simple)),
            Some('t') => Ok(('\t' as u128, Simple)),
            Some('v') => Ok(('\u{B}' as u128, Simple)),
            Some('f') => Ok(('\u{C}' as u128, Simple)),
            Some('a') => Ok(('\u{7}' as u128, Simple)),
            Some('b') => Ok(('\u{8}' as u128, Simple)),
            Some('\\') => Ok(('\\' as u128, Simple)),
            Some('?') => Ok(('?' as u128, Simple)),
            Some('\'') => Ok(('\'' as u128, Simple)),
            Some('"') => Ok(('"' as u128, Simple)),

            Some('o') => {
                if it.clone().next() == Some('{') {
                    eat_digits_in_braces(&mut it, 8).map(|v| (v, Numeric))
                } else {
                    Err(EscapeError::ExpectedOpenBrace)
                }
            }

            Some('x') => {
                let digits = match it.clone().next() {
                    Some(c) if c.is_ascii_hexdigit() => eat_digits(&mut it, None, 16),
                    Some('{') => eat_digits_in_braces(&mut it, 16),
                    _ => Err(EscapeError::ExpectedOpenBraceOrHexDigit),
                };

                digits.map(|v| (v, Numeric))
            }

            Some(c) if let Some(digit) = c.to_digit(8) => {
                let mut value = digit as u128;
                for _ in 0..2 {
                    if let Some(digit) = it.clone().next().and_then(|c| c.to_digit(8)) {
                        it.next();
                        value = value * 8 + digit as u128;
                    } else {
                        break;
                    }
                }

                Ok((value, Numeric))
            }

            _ => Err(EscapeError::UnknownEscape),
        };

        if res.is_ok() {
            *chars = it;
        }
        Some(res)
    } else {
        None
    }
}

fn eat_digits(chars: &mut SkipLineCont, n: Option<u32>, base: u32) -> Result<u128, EscapeError> {
    let mut it = chars.clone();
    let mut value = 0;
    let max = n.unwrap_or(u32::MAX);
    for _ in 0..max {
        if let Some(digit) = it.clone().next().and_then(|c| c.to_digit(base)) {
            it.next();
            value = value * base as u128 + digit as u128;
        } else {
            if let Some(n) = n {
                return Err(EscapeError::ExpectedDigits { n, base });
            }
            break;
        }
    }

    *chars = it;
    Ok(value)
}

fn eat_digits_in_braces(chars: &mut SkipLineCont, base: u32) -> Result<u128, EscapeError> {
    let mut it = chars.clone();
    let mut value = 0;
    let mut empty = true;
    let mut invalid_digit = false;
    let mut saw_close_brace = false;

    debug_assert_eq!(it.clone().next(), Some('{'));
    it.next();

    loop {
        match it.next() {
            Some('}') => {
                saw_close_brace = true;
                if empty {
                    return Err(EscapeError::EmptyBraces);
                }
                break;
            }
            Some(c) if let Some(digit) = c.to_digit(base) => {
                empty = false;
                value = value * base as u128 + digit as u128
            }
            Some(_) => invalid_digit = true,
            None => break,
        }
    }

    if !saw_close_brace {
        return Err(EscapeError::NoCloseBrace);
    }
    if invalid_digit {
        return Err(EscapeError::InvalidDigitInBraces { base });
    }

    *chars = it;
    Ok(value)
}

fn eat_ucn(chars: &mut SkipLineCont) -> Option<Result<char, EscapeError>> {
    let value_to_char = |v| {
        u32::try_from(v)
            .map_err(|_| EscapeError::InvalidUcnValue)
            .and_then(|v| char::from_u32(v).ok_or(EscapeError::InvalidUcnValue))
    };

    let mut it = chars.clone();
    if it.next() == Some('\\') {
        let res = match it.next() {
            Some('u') => {
                let digits = if it.clone().next() == Some('{') {
                    eat_digits_in_braces(&mut it, 16)
                } else {
                    eat_digits(&mut it, Some(4), 16)
                };

                digits.and_then(value_to_char)
            }

            Some('U') => eat_digits(&mut it, Some(8), 16).and_then(value_to_char),

            Some('N') => {
                if it.next() == Some('{') {
                    eat_ucn_name(&mut it)
                } else {
                    Err(EscapeError::ExpectedOpenBrace)
                }
            }

            _ => {
                // on a pas reconnu de UCN mais ce n'est pas une erreur pour
                // autant, ça pourrait être une autre escape sequence
                return None;
            }
        };

        if res.is_ok() {
            *chars = it;
        }
        Some(res)
    } else {
        None
    }
}

fn eat_ucn_name(chars: &mut SkipLineCont) -> Result<char, EscapeError> {
    let mut saw_close_brace = false;
    // todo: temp alloc
    let name = chars
        .inspect(|&c| saw_close_brace = c == '}')
        .take_while(|&c| c != '}')
        .collect::<String>();

    if !saw_close_brace {
        Err(EscapeError::NoCloseBrace)
    } else if name.is_empty() {
        Err(EscapeError::EmptyBraces)
    } else {
        if name.contains(|c| !matches!(c, 'A'..='Z' | '0'..='9' | ' ' | '-')) {
            // unicode_names2 autorise le loose matching mais pas le standard C++
            // il me semble
            // il y a peut-être d'autres cas à interdire mais bon ça ira hein
            Err(EscapeError::InvalidUcnName)
        } else {
            unicode_names2::character(&name).ok_or(EscapeError::InvalidUcnName)
        }
    }
}

pub struct Lexer<'a> {
    src: &'a str,
    chars: SkipLineCont<'a>,
    start: u32,
    errors: Vec<LexError>,
    #[cfg(debug_assertions)]
    prev: char,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            chars: SkipLineCont { raw: src.chars() },
            start: 0,
            errors: Vec::new(),
            #[cfg(debug_assertions)]
            prev: '\0',
        }
    }

    pub fn errors(&self) -> &Vec<LexError> {
        &self.errors
    }

    pub fn lex(&mut self) -> (TokenKind, Range<u32>) {
        use TokenKind::*;

        self.eat_whitespace();
        self.start = self.pos();

        let kind = match self.bump() {
            Some('#') if self.eat('#') => HashHash,
            Some('#') => Hash,

            Some('%') if self.eat(':') => {
                if self.eat_two('%', ':') {
                    HashHash
                } else {
                    Hash
                }
            }
            Some('%') if self.eat('=') => PercentEq,
            Some('%') if self.eat('>') => BraceR,
            Some('%') => Percent,

            Some('[')
                if self.peek(0) == ':'
                    && (self.peek(1) != ':' || self.peek(2) == ':')
                    && self.peek(1) != '>' =>
            {
                self.bump();
                SpliceL
            }
            Some('[') => BracketL,

            Some(']') => BracketR,
            Some('{') => BraceL,
            Some('}') => BraceR,
            Some('(') => ParenL,
            Some(')') => ParenR,
            Some(';') => Semi,
            Some(',') => Comma,
            Some('?') => Question,
            Some('~') => Tilde,

            Some(':') if self.eat(']') => SpliceR,
            Some(':') if self.eat('>') => BracketR,
            Some(':') if self.eat(':') => ColonColon,
            Some(':') => Colon,

            Some('>') if self.eat_two('>', '=') => GtGtEq,
            Some('>') if self.eat('>') => GtGt,
            Some('>') if self.eat('=') => GtEq,
            Some('>') => Gt,

            Some('<') if self.eat_two('<', '=') => LtLtEq,
            Some('<') if self.eat_two('=', '>') => Spaceship,
            Some('<') if self.eat('<') => LtLt,
            Some('<') if self.eat('=') => LtEq,
            Some('<') if self.eat('%') => BraceL,
            Some('<')
                if self.peek(0) == ':'
                    && (self.peek(1) != ':' || self.peek(2) == ':' || self.peek(2) == '>') =>
            {
                self.bump();
                BracketL
            }
            Some('<') => Lt,

            Some('+') if self.eat('+') => PlusPlus,
            Some('+') if self.eat('=') => PlusEq,
            Some('+') => Plus,

            Some('-') if self.eat_two('>', '*') => ArrowStar,
            Some('-') if self.eat('>') => Arrow,
            Some('-') if self.eat('=') => MinusEq,
            Some('-') if self.eat('-') => MinusMinus,
            Some('-') => Minus,

            Some('!') if self.eat('=') => Ne,
            Some('!') => Bang,

            Some('*') if self.eat('=') => StarEq,
            Some('*') => Star,

            Some('/') if self.eat('=') => SlashEq,
            Some('/') => Slash,

            Some('^') if self.eat('^') => CatEars,
            Some('^') if self.eat('=') => CaretEq,
            Some('^') => Caret,

            Some('&') if self.eat('=') => AndEq,
            Some('&') if self.eat('&') => AndAnd,
            Some('&') => And,

            Some('|') if self.eat('=') => OrEq,
            Some('|') if self.eat('|') => OrOr,
            Some('|') => Or,

            Some('=') if self.eat('=') => EqEq,
            Some('=') => Eq,

            Some('\'') => self.char(""),
            Some('"') => self.str(""),

            Some('.') if self.peek(0).is_ascii_digit() => self.number(),
            Some('.') if self.eat_two('.', '.') => DotDotDot,
            Some('.') if self.eat('*') => DotStar,
            Some('.') => Dot,

            Some(c) if c.is_ascii_digit() => self.number(),
            Some(c) if is_ident_start(c) => self.name_or_prefix(),

            Some(_) => Unknown,
            None => Eof,
        };

        (kind, self.start..self.pos())
    }

    pub fn bump(&mut self) -> Option<char> {
        let ucn_start = self.pos();
        let next = match eat_ucn(&mut self.chars) {
            Some(Ok(c)) => {
                if is_basic_charset(c) || c.is_control() {
                    self.errors.push(LexError::UnexpectedBasicUcn {
                        c,
                        is_control: c.is_control(),
                        // todo: ça serait mieux d'avoir le span de l'UCN en lui-même
                        // et pas juste le premier caractère
                        span: ucn_start..ucn_start + 1,
                    });
                }
                Some(c)
            }

            Some(Err(e)) => {
                self.errors
                    .push(LexError::Escape(e, ucn_start..ucn_start + 1));
                self.chars.next()
            }

            None => self.chars.next(),
        };

        #[cfg(debug_assertions)]
        {
            self.prev = next.unwrap_or_default();
        }

        next
    }

    pub fn peek(&self, ahead: u32) -> char {
        let it = &mut self.chars.clone();
        for _ in 0..ahead {
            eat_ucn(it);
            it.next();
        }

        if let Some(Ok(c)) = eat_ucn(it) {
            c
        } else {
            it.next().unwrap_or_default()
        }
    }

    pub fn eat(&mut self, c: char) -> bool {
        if self.peek(0) == c {
            self.bump();
            true
        } else {
            false
        }
    }

    pub fn eat_two(&mut self, c0: char, c1: char) -> bool {
        if self.peek(0) == c0 && self.peek(1) == c1 {
            self.bump();
            self.bump();
            true
        } else {
            false
        }
    }

    pub fn eof(&self) -> bool {
        self.chars.len() == 0
    }

    pub fn pos(&self) -> u32 {
        (self.src.len() - self.chars.len()) as u32
    }

    fn name_or_prefix(&mut self) -> TokenKind {
        debug_assert!(is_ident_start(self.prev));

        while is_ident_continue(self.peek(0)) {
            self.bump();
        }

        let ident = extract_lexeme_basic(&self.src[self.start as usize..self.pos() as usize]);

        match self.peek(0) {
            '\'' if is_char_prefix(&ident) => {
                self.bump();
                self.char(&ident)
            }
            '"' if is_str_prefix(&ident) => {
                self.bump();
                self.str(&ident)
            }
            _ => to_alt_token(&ident).unwrap_or_else(|| TokenKind::Name(Name::from(&ident))),
        }
    }

    fn char(&mut self, prefix: &str) -> TokenKind {
        debug_assert_eq!(self.prev, '\'');

        let encoding = Encoding::from_prefix(prefix);
        let mut chars = ArrayVec::<StrChar, MAX_MULTICHAR_LEN>::new();
        let mut has_too_many_chars = false;
        let mut has_invalid_escape = false;
        let mut is_terminated = false;

        loop {
            if eat_newline(&mut self.chars.clone()) {
                // le newline ne fait pas partie du char
                break;
            }

            // on ne peut pas utiliser `self.bump` pour récupérer le `'` fermant
            // car sinon on pourrait le confondre avec `\u0027`
            if let Some('\'') = self.chars.clone().next() {
                self.chars.next();
                is_terminated = true;
                break;
            }

            if let Some(c) = self.next_char_in_str(encoding) {
                has_invalid_escape |= c.is_invalid_escape;
                if chars.try_push(c).is_err() {
                    has_too_many_chars = true;
                }
            } else {
                break;
            }
        }

        if !is_terminated {
            self.errors
                .push(LexError::Unterminated(UnterminatedKind::Char, self.start));
        } else if has_too_many_chars && !has_invalid_escape {
            self.errors.push(LexError::Char(
                CharError::TooManyChars,
                self.start..self.pos(),
            ));
        }

        let ud_suffix = self.str_suffix(false);

        match &chars[..] {
            [] => {
                if is_terminated {
                    self.errors
                        .push(LexError::Char(CharError::Empty, self.start..self.pos()));
                }

                TokenKind::Char(encoding, '\0' as u32, None)
            }

            [c] => {
                if !c.is_numeric_escape {
                    let is_single_code_unit = |c: char| match encoding {
                        Encoding::Ordinary | Encoding::Utf8 => c.is_ascii(),
                        Encoding::Wide | Encoding::Utf16 => c.len_utf16() == 1,
                        Encoding::Utf32 => true,
                    };

                    if !char::from_u32(c.value).is_some_and(is_single_code_unit) {
                        self.errors.push(LexError::Char(
                            CharError::Unmappable,
                            self.start..self.pos(),
                        ));
                    }
                }

                TokenKind::Char(encoding, c.value, ud_suffix)
            }

            _ => {
                if !prefix.is_empty() && !has_invalid_escape {
                    self.errors.push(LexError::Char(
                        CharError::MulticharPrefix,
                        self.start..self.pos(),
                    ));
                }

                let mut value = 0;
                for (i, c) in chars.iter().rev().enumerate() {
                    if char::from_u32(c.value).is_some_and(|c| c.is_ascii()) {
                        value |= c.value << (8 * i as u32);
                    } else if !c.is_invalid_escape {
                        self.errors.push(LexError::Char(
                            CharError::NonAsciiInMultichar,
                            self.start..self.pos(),
                        ));
                    }
                }

                TokenKind::Multichar(value, ud_suffix)
            }
        }
    }

    fn str(&mut self, prefix: &str) -> TokenKind {
        debug_assert_eq!(self.prev, '"');

        let is_raw = prefix.chars().last().is_some_and(|c| c == 'R');
        let prefix_end = if is_raw {
            prefix.len() - 1
        } else {
            prefix.len()
        };
        let encoding = Encoding::from_prefix(&prefix[..prefix_end]);

        if is_raw {
            self.raw_str(encoding)
        } else {
            self.non_raw_str(encoding)
        }
    }

    fn raw_str(&mut self, encoding: Encoding) -> TokenKind {
        debug_assert_eq!(self.prev, '"');

        const MAX_DELIM_LEN: usize = 16;

        let delim_start = self.pos();
        let mut invalid_char_in_delim = false;
        let mut found_delim = false;

        // on utilise les chars raw car on ne veut pas manger de line
        // continuation ni UCN
        let it = &mut self.chars.raw;

        // todo: ça serait bien d'utiliser une SmallString ou un truc du genre
        // pour ne pas avoir à allouer à chaque fois (mais on veut quand même
        // que ça puisse fallback sur la heap pour qu'on puisse continuer
        // le lexing même si le delim est trop grand)
        let mut delim = String::with_capacity(MAX_DELIM_LEN);
        let mut pos = delim_start;
        loop {
            if eat_newline(it) {
                self.errors
                    .push(LexError::Str(StrError::InvalidCharInDelim, pos..pos + 1));
                invalid_char_in_delim = true;
                break;
            }

            match it.next() {
                Some('(') => {
                    found_delim = true;
                    break;
                }
                Some(c) => {
                    if matches!(c, ' ' | ')' | '\\' | '\t' | '\u{B}' | '\u{C}')
                        || !is_basic_charset(c)
                    {
                        self.errors
                            .push(LexError::Str(StrError::InvalidCharInDelim, pos..pos + 1));
                        invalid_char_in_delim = true;
                        break;
                    }

                    delim.push(c);
                }
                None => break,
            }

            pos += 1;
        }

        if !found_delim {
            // tous les caractères qu'on a vu jusque là ne peuvent pas être
            // le délim puisqu'on a pas vu le `(` donc c'est pas le délim
            delim.clear();
        }

        if invalid_char_in_delim {
            // c'est foutu, on essaie de trouver la fin de la chaîne en
            // cherchant un `"` mais on peut se tromper si il y en a en plein
            // milieu de la chaîne mais comme on a pas le délim pour nous
            // aider on fait ce qu'on peut
            for _ in it.take_while(|&c| c != '"') {}

            return TokenKind::Str(StrKind::Raw, encoding, ByteString(vec![0]), None);
        }

        if delim.len() >= MAX_DELIM_LEN {
            self.errors.push(LexError::Str(
                StrError::TooManyCharsInDelim,
                delim_start..pos,
            ));
        }

        let mut is_terminated = false;
        let mut value = ByteString(Vec::new());
        loop {
            match it.next() {
                Some(')') => {
                    let old = it.clone();
                    let matched_delim_len = delim
                        .chars()
                        .zip(it.by_ref())
                        .take_while(|(a, b)| a == b)
                        .count();

                    if matched_delim_len == delim.len() && it.next() == Some('"') {
                        is_terminated = true;
                        break;
                    } else {
                        // on s'est fait avoir, ce n'était pas la fin de la chaine...
                        *it = old;
                        push_char(&mut value, ')', encoding);
                        for c in it.take(matched_delim_len) {
                            push_char(&mut value, c, encoding);
                        }
                    }
                }

                Some(c) => push_char(&mut value, c, encoding),
                None => break,
            }
        }

        if !is_terminated {
            self.errors.push(LexError::Unterminated(
                UnterminatedKind::RawStr {
                    delim: if delim.is_empty() {
                        None
                    } else {
                        Some(delim.clone())
                    },
                },
                self.start,
            ));
        }

        value.push(0);
        let ud_suffix = self.str_suffix(true);

        TokenKind::Str(StrKind::Raw, encoding, value, ud_suffix)
    }

    fn non_raw_str(&mut self, encoding: Encoding) -> TokenKind {
        debug_assert_eq!(self.prev, '"');

        let mut is_terminated = false;
        let mut value = ByteString(Vec::new());
        loop {
            if eat_newline(&mut self.chars.clone()) {
                break;
            }

            // on ne peut pas utiliser `self.bump` pour récupérer le `"`
            // car sinon on pourrait le confondre avec `\u0022`
            if let Some('"') = self.chars.clone().next() {
                self.chars.next();
                is_terminated = true;
                break;
            }

            if let Some(c) = self.next_char_in_str(encoding) {
                if c.is_numeric_escape {
                    match encoding {
                        Encoding::Ordinary | Encoding::Utf8 => value.push(c.value as u8),
                        Encoding::Wide | Encoding::Utf16 => {
                            value.extend_from_slice(&(c.value as u16).to_le_bytes())
                        }
                        Encoding::Utf32 => value.extend_from_slice(&c.value.to_le_bytes()),
                    }
                } else {
                    // SAFETY: le next char est forcément un char sauf si c'est
                    // une numeric escape donc ça devrait être valide
                    push_char(
                        &mut value,
                        unsafe { char::from_u32_unchecked(c.value) },
                        encoding,
                    );
                }
            } else {
                break;
            }
        }

        if !is_terminated {
            self.errors
                .push(LexError::Unterminated(UnterminatedKind::Str, self.start));
        }

        value.push(0);
        let ud_suffix = self.str_suffix(false);

        TokenKind::Str(StrKind::NonRaw, encoding, value, ud_suffix)
    }

    fn next_char_in_str(&mut self, encoding: Encoding) -> Option<StrChar> {
        let escape_start = self.pos();
        if let Some(ucn) = eat_ucn(&mut self.chars) {
            match ucn {
                Ok(c) => Some(StrChar {
                    value: c as u32,
                    is_numeric_escape: false,
                    is_invalid_escape: false,
                }),
                Err(e) => {
                    self.errors
                        .push(LexError::Escape(e, escape_start..escape_start + 1));

                    // on retourne le prochain caractère (premier caractère de l'UCN),
                    // ce qui revient à interpréter l'UCN invalide caractère par caractère
                    self.chars.next().map(|c| StrChar {
                        value: c as u32,
                        is_numeric_escape: false,
                        is_invalid_escape: true,
                    })
                }
            }
        } else {
            match eat_escape_seq(&mut self.chars) {
                Some(Ok((v, kind))) => {
                    let mut has_escape_error = false;
                    if kind == EscapeSeqKind::Numeric {
                        let max = match encoding {
                            Encoding::Ordinary | Encoding::Utf8 => u8::MAX as u128,
                            Encoding::Wide | Encoding::Utf16 => u16::MAX as u128,
                            Encoding::Utf32 => u32::MAX as u128,
                        };
                        if v > max {
                            self.errors.push(LexError::NumericEscapeOutOfRange(
                                escape_start..escape_start + 1,
                            ));
                            has_escape_error = true;
                        }
                    };

                    Some(StrChar {
                        value: v as u32,
                        is_numeric_escape: kind == EscapeSeqKind::Numeric,
                        is_invalid_escape: has_escape_error,
                    })
                }

                Some(Err(e)) => {
                    self.errors
                        .push(LexError::Escape(e, escape_start..escape_start + 1));

                    self.chars.next().map(|c| StrChar {
                        value: c as u32,
                        is_numeric_escape: false,
                        is_invalid_escape: true,
                    })
                }

                // on a pas mangé d'escape sequence donc on retourne juste le
                // prochain caractère
                None => self.chars.next().map(|c| StrChar {
                    value: c as u32,
                    is_numeric_escape: false,
                    is_invalid_escape: false,
                }),
            }
        }
    }

    fn number(&mut self) -> TokenKind {
        debug_assert_matches!(self.prev, '0'..='9' | '.');

        loop {
            match self.peek(0) {
                '\'' if matches!(self.peek(1), '0'..='9' | 'a'..='z' | 'A'..='Z' | '_') => {
                    self.bump();
                }
                'e' | 'E' | 'p' | 'P' if matches!(self.peek(1), '+' | '-') => {
                    self.bump();
                }
                '.' => {}
                c if is_ident_continue(c) => {}
                _ => break,
            }

            self.bump();
        }

        TokenKind::Number
    }

    fn str_suffix(&mut self, is_raw_str: bool) -> Option<UdSuffix> {
        if is_ident_start(self.peek(0)) {
            // ok il y a un suffixe mais on veut sa position dans le lexème lui-même
            // (donc sans line continuations etc)
            let src = &self.src[self.start as usize..self.pos() as usize];
            let pos = if is_raw_str {
                extract_lexeme_raw_str(src)
            } else {
                extract_lexeme_basic(src)
            }
            .len();

            self.bump();
            while is_ident_continue(self.peek(0)) {
                self.bump();
            }

            Some(NonZeroU32::new(pos as u32).expect("pos should be > 0"))
        } else {
            None
        }
    }

    fn eat_whitespace(&mut self) {
        loop {
            // les line continuations seraient mangées automatiquement mais on
            // veut les manger explicitement maintenant car juste après on
            // récupère la position du début du token et on veut pas inclure
            // les line continuations
            eat_line_cont(&mut self.chars.raw);

            match self.peek(0) {
                '/' => match self.peek(1) {
                    '/' => self.line_comment(),
                    '*' => self.multiline_comment(),
                    _ => return,
                },
                c if is_whitespace(c) => {
                    self.bump();
                }
                _ => return,
            }
        }
    }

    fn line_comment(&mut self) {
        debug_assert_eq!(self.peek(0), '/');
        debug_assert_eq!(self.peek(1), '/');
        self.chars.next();
        self.chars.next();

        // todo: on pourrait chercher le newline directement dans les bytes
        while !self.eof() {
            if eat_newline(&mut self.chars) {
                return;
            }
            self.chars.next();
        }
    }

    fn multiline_comment(&mut self) {
        debug_assert_eq!(self.peek(0), '/');
        debug_assert_eq!(self.peek(1), '*');

        let start = self.pos();
        self.chars.next();
        self.chars.next();

        // todo: on pourrait chercher le `*` dans les bytes
        while !self.eof() {
            if self.eat_two('*', '/') {
                return;
            }
            self.chars.next();
        }

        self.errors.push(LexError::Unterminated(
            UnterminatedKind::MultilineComment,
            start,
        ));
    }
}

fn push_char(dst: &mut ByteString, c: char, encoding: Encoding) {
    match encoding {
        Encoding::Ordinary | Encoding::Utf8 => encode_utf8(c, &mut dst.0),
        Encoding::Wide | Encoding::Utf16 => encode_utf16(c, &mut dst.0),
        Encoding::Utf32 => encode_utf32(c, &mut dst.0),
    }
}

struct StrChar {
    value: u32,
    is_numeric_escape: bool,
    is_invalid_escape: bool,
}
