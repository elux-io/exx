//! ~ preprocessing ~

use crate::{
    diag::{Diag, DiagKind, DiagPart, Diags, Squiggle},
    lex::{
        CharError, Encoding, EscapeError, HeaderKind, LexError, Lexer, MAX_MULTICHAR_LEN,
        MAX_RAW_STR_DELIM_LEN, NumberLit, NumberLitKind, ParseNumberError, SkipLineCont, StrError,
        StrKind, Token, TokenKind, UnterminatedKind, eat_newline, parse_number, to_span,
    },
    name::{
        Name, attr_kw, kw,
        pp_kw::{self},
    },
    source::{
        FileLoader, FileName, FileStatus, LineRemap, LoadError, Loc, LocOrigin, SourceHub,
        SourceId, Span, SubstKind,
    },
};
use chrono::Utc;
use core::{
    debug_assert_matches,
    iter::{IntoIterator, Iterator},
};
use std::{
    borrow::Cow,
    bstr::ByteStr,
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque, hash_map::Entry},
    fmt::Write,
    hint::{cold_path, unlikely},
    ops::Range,
    path::{Path, PathBuf},
    slice::Iter,
    str::Chars,
};

pub struct PpOptions {
    /// profondeur max d'inclusion
    pub max_include_depth: u32,
    /// active les defines "communs" non-standards (__COUNTER__, ...)
    pub common_defines: bool,
    /// active le support de #pragma once
    pub pragma_once: bool,
}

impl Default for PpOptions {
    fn default() -> Self {
        Self {
            max_include_depth: 256,
            common_defines: false,
            pragma_once: false,
        }
    }
}

pub struct Preprocessor<'a> {
    pub shub: &'a mut SourceHub,
    pub diags: &'a mut Diags,
    pub file_loader: &'a dyn FileLoader,
    opts: PpOptions,
    include_paths: HeaderPaths,
    embed_paths: HeaderPaths,
    // todo: FxHashMap?
    mac_table: HashMap<Name, Mac>,
    attrs: Vec<Attr>,
    counter: u32,
    pragma_once_paths: HashSet<PathBuf>,
}

impl<'a> Preprocessor<'a> {
    pub fn new(
        opts: PpOptions,
        shub: &'a mut SourceHub,
        diags: &'a mut Diags,
        file_loader: &'a dyn FileLoader,
    ) -> Self {
        let mut pp = Self {
            opts,
            include_paths: HeaderPaths::default(),
            embed_paths: HeaderPaths::default(),
            attrs: standard_attrs(shub),
            shub,
            diags,
            file_loader,
            mac_table: HashMap::new(),
            counter: 0,
            pragma_once_paths: HashSet::new(),
        };
        pp.add_predefined_macs();
        pp
    }

    pub fn preprocess(&'a mut self, source_id: SourceId) -> Vec<Token> {
        let mut tokens = Vec::new();
        let source_start = self.shub.source(source_id).start();
        InnerPreprocessor::new(self, source_id, source_start, 0, &mut tokens).preprocess();
        tokens
    }

    pub fn add_define(&mut self, name: impl Into<Name>, body: &str) {
        self.add_define_impl(name.into(), body, false);
    }

    pub fn add_include_dir(&mut self, path: PathBuf, kind: HeaderKind) {
        // todo: EnumMap?
        match kind {
            HeaderKind::Angle => self.include_paths.angle.push(path),
            HeaderKind::Quote => self.include_paths.quote.push(path),
        }
    }

    pub fn add_embed_dir(&mut self, path: PathBuf, kind: HeaderKind) {
        match kind {
            HeaderKind::Angle => self.embed_paths.angle.push(path),
            HeaderKind::Quote => self.embed_paths.quote.push(path),
        }
    }

    fn add_define_impl(&mut self, name: Name, body: &str, predefined: bool) {
        assert!(!self.mac_table.contains_key(&name));

        let mut lexer = Lexer::new(body, self.shub.write_virtual_source(body));
        lexer.set_at_bol(false);

        let body = parse_mac_body(false, &mut lexer, self.diags);

        for e in lexer.errors() {
            // todo: pas clone
            self.diags.emit(e.clone());
        }

        self.mac_table.insert(
            name,
            Mac {
                kind: MacKind::Obj,
                body,
                name_span: None,
                predefined,
                expanding: false,
            },
        );
    }

    fn add_predefined_macs(&mut self) {
        let now = Utc::now();
        let date = now.format("\"%b %e %Y\"").to_string();
        let time = now.format("\"%H:%M:%S\"").to_string();
        let standard_defines = [
            ("__cplusplus", "202600L"), // todo: mettre la vraie valeur une fois connue
            ("__STDC_EMBED_NOT_FOUND__", "0"),
            ("__STDC_EMBED_FOUND__", "1"),
            ("__STDC_EMBED_EMPTY__", "2"),
            ("__STDC_HOSTED__", "0"),
            ("__STDCPP_DEFAULT_NEW_ALIGNMENT__", "16uz"),
            ("__DATE__", &date),
            ("__TIME__", &time),
        ];

        let common_defines = [("__EXX__", "1")];

        for (name, body) in standard_defines {
            self.add_define_impl(Name::from(name), body, true);
        }

        if self.opts.common_defines {
            for (name, body) in common_defines {
                self.add_define_impl(Name::from(name), body, true);
            }
        }

        let mut add_builtin = |name, builtin| {
            self.mac_table.insert(
                Name::from(name),
                Mac {
                    kind: MacKind::Builtin(builtin),
                    body: None,
                    name_span: None,
                    expanding: false,
                    predefined: true,
                },
            )
        };

        add_builtin("__LINE__", BuiltinMac::Line);
        add_builtin("__FILE__", BuiltinMac::File);
        add_builtin("__has_cpp_attribute", BuiltinMac::HasExpr);
        add_builtin("__has_embed", BuiltinMac::HasExpr);
        add_builtin("__has_include", BuiltinMac::HasExpr);
        add_builtin("_Pragma", BuiltinMac::PragmaOp);

        if self.opts.common_defines {
            add_builtin("__BASE_FILE__", BuiltinMac::BaseFile);
            add_builtin("__FILE_NAME__", BuiltinMac::FileName);
            add_builtin("__COUNTER__", BuiltinMac::Counter);
            add_builtin("__TIMESTAMP__", BuiltinMac::Timestamp);
            add_builtin("__INCLUDE_LEVEL__", BuiltinMac::IncludeLevel);
        }
    }

    fn resolve_header(
        &self,
        group: HeaderGroup,
        name: &str,
        kind: HeaderKind,
        relative_to: SourceId,
    ) -> Option<(PathBuf, FileStatus)> {
        let paths = match group {
            HeaderGroup::Include => &self.include_paths,
            HeaderGroup::Embed => &self.embed_paths,
        };

        let header_path = Path::new(name);

        let check = |parent: &Path| {
            let full_path = parent.join(header_path).normalize_lexically().ok()?;
            let status = self.file_loader.status(&full_path);
            match status {
                FileStatus::Invalid(LoadError::NotFound) => None,
                _ => Some((full_path, status)),
            }
        };

        if kind == HeaderKind::Quote {
            let parent = self.shub.source(relative_to).path().parent().expect("");
            if let Some(res) = check(parent) {
                return Some(res);
            }

            // on tente le header_path tout seul, sans parent
            // c'est pour gérer le cas où le chemin est "absolu" sans l'être
            // officiellement, normalement ça sert à rien car si on met un chemin
            // absolu il écrase le parent (lors du join) donc ça serait pris en charge
            // dans le cas précédent mais le concept de chemin absolu dépend
            // de la platforme et le FileLoader est platform-agnostic, donc si
            // on veut mettre comme "root" un chemin qui n'est pas considéré
            // "absolu" il faut quand même que ça marche, par exemple si on
            // considère que le fichier principal est `root/src/main.cpp` et que
            // dedans on veut #include "root/src/bla.hpp" (donc un chemin qu'on
            // considère comme "absolu"), alors le cas prédédent ne marchera
            // pas (il chercherait le fichier `root/src/root/src/bla.hpp`)
            if let Some(res) = check(Path::new("")) {
                return Some(res);
            }

            if let Some(res) = paths.quote.iter().map(|p| p.as_path()).find_map(check) {
                return Some(res);
            }
        }

        paths.angle.iter().map(|p| p.as_path()).find_map(check)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderGroup {
    Include,
    Embed,
}

#[derive(Default)]
struct HeaderPaths {
    angle: Vec<PathBuf>,
    quote: Vec<PathBuf>,
}

fn standard_attrs(shub: &mut SourceHub) -> Vec<Attr> {
    // normalement il faudrait les mettre à 0 parce que c'est pas encore vraiment
    // supporté mais bon
    let attrs = [
        (attr_kw::Assume, "202207L"),
        (attr_kw::Deprecated, "201309L"),
        (attr_kw::Fallthrough, "201603L"),
        (attr_kw::Indeterminate, "202403L"),
        (attr_kw::Likely, "201803L"),
        (attr_kw::MaybeUnused, "201603L"),
        (attr_kw::NoUniqueAddress, "201803L"),
        (attr_kw::Nodiscard, "201907L"),
        (attr_kw::Noreturn, "200809L"),
        (attr_kw::Unlikely, "201803L"),
    ];

    attrs
        .into_iter()
        .map(|(name, value)| Attr::new(None, name, value, shub))
        .collect()
}

#[derive(Clone, PartialEq)]
struct MacParam {
    name: Name,
    requires_expansion: bool,
}

#[derive(Clone)]
struct MacArg {
    tokens: Vec<Token>,
    expanded: Option<Vec<Token>>,
    contains_names: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum BuiltinMac {
    Line,         // __LINE__
    File,         // __FILE__
    HasExpr,      // __has_include, __has_embed, ...
    PragmaOp,     // _Pragma
    FileName,     // __FILE_NAME__
    BaseFile,     // __BASE_FILE__
    Counter,      // __COUNTER__
    Timestamp,    // __TIMESTAMP__
    IncludeLevel, // __INCLUDE_LEVEL__
}

#[derive(Clone, PartialEq)]
enum MacKind {
    Obj,
    Fn {
        params: Vec<MacParam>,
        variadic: bool,
    },
    Builtin(BuiltinMac),
}

#[derive(Clone)]
struct MacBody {
    tokens: Vec<Token>,
    has_concats: bool,
    span: Span,
}

struct Mac {
    kind: MacKind,
    body: Option<MacBody>,
    name_span: Option<Span>,
    predefined: bool,
    expanding: bool,
}

fn is_valid_redefinition(old: &Mac, new: &Mac, shub: &SourceHub) -> bool {
    if old.kind != new.kind {
        return false;
    }

    if let Some(old) = &old.body
        && let Some(new) = &new.body
        && let Some((old_first, old_rest)) = old.tokens.split_first()
        && let Some((new_first, new_rest)) = new.tokens.split_first()
    {
        old_first.kind == new_first.kind
            && old_first.lexeme(shub) == new_first.lexeme(shub)
            && old_rest.iter().eq_by(new_rest, |a, b| {
                a.kind == b.kind
                    && a.lexeme(shub) == b.lexeme(shub)
                    && a.space_before == b.space_before
            })
    } else {
        old.body.is_none() && new.body.is_none()
    }
}

struct Attr {
    namespace: Option<Name>,
    name: Name,
    token: Token,
}

impl Attr {
    fn new(namespace: Option<Name>, name: Name, value: &str, shub: &mut SourceHub) -> Self {
        let mut lexer = Lexer::new(value, shub.write_virtual_source(value));
        let token = lexer.lex();
        debug_assert!(lexer.errors().is_empty());

        Self {
            namespace,
            name,
            token,
        }
    }
}

/// retourne vrai si les 2 spans ont comme origine commune une même macro
fn come_from_same_mac(a: Span, b: Span, shub: &SourceHub) -> bool {
    let pred = |a_origin, b_origin| match (a_origin, b_origin) {
        (LocOrigin::Subst(i), LocOrigin::Subst(j)) => {
            i == j && matches!(shub.subst(i).kind, SubstKind::MacExpansion { .. })
        }
        _ => false,
    };

    shub.walk_up_pairwise_until(a, b, pred).is_some()
}

pub fn format_pp_output(tokens: &[Token], shub: &SourceHub) -> String {
    let mut out = String::with_capacity(tokens.len() * 3);
    let mut newlines = 0;
    let mut spaces = 0;
    let mut it = tokens.iter();

    while let Some(curr) = it.next() {
        for _ in 0..newlines {
            out.push('\n');
        }
        newlines = 0;

        for _ in 0..spaces {
            out.push(' ');
        }
        spaces = 0;

        // on peut pas juste afficher les tokens à la suite, on veut respecter les
        // newlines / spaces
        if let Some(next) = it.clone().next() {
            let ignore_newlines = come_from_same_mac(curr.span, next.span, shub);
            let (a, b) = shub.align_to_common_origin(curr.span, next.span);
            let curr_span = shub.source_span(a);
            let next_span = shub.source_span(b);
            let span = Span {
                lo: curr_span.hi.min(next_span.lo),
                hi: next_span.lo.max(curr_span.hi),
            };
            newlines = count_newlines(shub.text(span));

            if ignore_newlines {
                if newlines > 0 {
                    spaces = 1;
                }
                newlines = 0;
            }

            if newlines > 0 {
                // on sait qu'on va passer à la ligne au prochain token, donc on
                // compte le nombre de colonnes pour respecter l'indentation
                // todo: faire une fonction dans le source hub qui retourne
                // directement la partie gauche de la ligne ? (début de la ligne
                // jusqu'à telle position)
                let next_span = shub.walk_up_to_source(next.span);
                if let LocOrigin::Source(id) = shub.span_origin(next_span) {
                    let text_before = shub.text(Span {
                        lo: shub.source(id).start(),
                        hi: next_span.lo,
                    });

                    if let Some(i) = text_before.rfind(['\n', '\r']) {
                        spaces = text_before.len() - 1 - i;
                    }
                }
            } else {
                if next.space_before {
                    spaces = 1;
                }

                if unlikely(next.kind == TokenKind::Name(pp_kw::BuiltinPragma)) {
                    newlines = 1;
                }
            }

            if newlines == 0 && spaces == 0 {
                // on s'assure qu'on peut se permettre de ne pas mettre d'espace
                // (on veut éviter que ça forme un seul token alors qu'il y en
                // avait 2 à la base)
                let lexeme = [curr.lexeme(shub), next.lexeme(shub)].join("");
                let is_single_token = {
                    let mut lexer = Lexer::new(&lexeme, Loc(0));
                    lexer.lex();
                    lexer.lex().kind == TokenKind::Eof
                };

                // le token ... ne peut pas être détecté en regardant que 2
                // tokens à la fois donc on gère ce cas explicitement
                if is_single_token || lexeme == ".." {
                    spaces = 1;
                }
            }
        }

        match curr.kind {
            TokenKind::Name(pp_kw::BuiltinPragma) => {
                cold_path();
                out.push_str("#pragma");
                newlines = 0;
                spaces = 1;
            }
            TokenKind::Name(pp_kw::BuiltinPragmaEnd) => {
                cold_path();
                newlines = newlines.max(1);
            }
            _ => out.push_str(&curr.lexeme(shub)),
        }
    }

    out
}

fn count_newlines(s: &str) -> u32 {
    // on utilise pas `s.lines()` car on a pas la même définition de "newline"
    // on skip les line conts pour éviter d'avoir des newlines dans la sortie là
    // où se trouvaient des line conts (Clang fait pareil, GCC remplace les line
    // conts par des newlines)
    let mut count = 0;
    let mut it = SkipLineCont { raw: s.chars() };
    loop {
        while eat_newline(&mut it) {
            count += 1;
        }
        if it.next().is_none() {
            break;
        }
    }

    count
}

fn format_angle_header_name(tokens: &[Token], shub: &SourceHub) -> String {
    debug_assert!(tokens.first().is_some_and(|t| t.kind == TokenKind::Lt));
    debug_assert!(tokens.last().is_some_and(|t| t.kind == TokenKind::Gt));

    let mut out = String::new();
    let mut space = false;
    let mut it = tokens.iter();
    while let Some(curr) = it.next() {
        if space {
            out.push(' ');
        }
        space = it.clone().next().is_some_and(|t| t.space_before);
        out.push_str(&curr.lexeme(shub))
    }

    out
}

struct Cond {
    directive: Name,
    directive_span: Span,
    entered: bool,
    in_else: bool,
}

struct InnerPreprocessor<'a, 'b> {
    pp: &'b mut Preprocessor<'a>,
    tokens: &'b mut Vec<Token>,
    lexer: Lexer<'a>,
    source_id: SourceId,
    include_depth: u32,
    cond_stack: Vec<Cond>,
}

impl<'a, 'b> InnerPreprocessor<'a, 'b> {
    fn new(
        pp: &'b mut Preprocessor<'a>,
        source_id: SourceId,
        source_start: Loc,
        include_depth: u32,
        tokens: &'b mut Vec<Token>,
    ) -> Self {
        let source = pp.shub.source(source_id);
        Self {
            // SAFETY: le texte pointe dans le source hub et en même temps on
            // stocke un &mut du shub dans le pp (c'est pour ça que le borrow checke
            // est pas content) mais le texte lui-même n'est jamais modifié
            // todo: peut-être que c'est UB ? miri dit rien
            lexer: Lexer::new(unsafe { &*std::ptr::from_ref(source.text()) }, source_start),
            tokens,
            source_id,
            pp,
            include_depth,
            cond_stack: Vec::new(),
        }
    }

    fn preprocess(&mut self) {
        // ignore BOM
        self.lexer.eat('\u{FEFF}');

        let mut next_needs_space = false;
        while !self.lexer.eof() {
            let at_bol = self.lexer.at_bol();
            let mut token = self.lexer.lex();
            token.space_before |= next_needs_space;
            next_needs_space = false;

            match token.kind {
                TokenKind::Hash if at_bol => self.directive(token.span),
                TokenKind::Name(name) if !name.is_kw() && self.pp.mac_table.contains_key(&name) => {
                    // on a vérifié que c'était pas un kw et que c'était une macro
                    // mais c'est pas nécessaire car ça sera aussi vérifié par
                    // l'expander mais c'est pour essayer de faire le moins de
                    // travail possible dans le cas où c'est pas une macro, car
                    // c'est de loin le cas le plus fréquent
                    let mut expander = MacExpander::new(self.pp, true);
                    let start = self.tokens.len();
                    expander.expand(
                        self.tokens,
                        token,
                        Some(&mut self.lexer),
                        &mut next_needs_space,
                    );

                    if expander.saw_pragma {
                        self.handle_pragma_ops(start);
                    }
                }
                _ => self.tokens.push(token),
            }
        }

        for cond in &self.cond_stack {
            self.pp.diags.emit(NoEndif {
                directive: cond.directive,
                span: cond.directive_span,
            });
        }

        for e in self.lexer.errors() {
            // todo: pas clone
            self.pp.diags.emit(e.clone());
        }
    }

    fn directive(&mut self, hash_span: Span) {
        if self.lexer.at_bol() {
            // null directive
            return;
        }

        let token = self.lexer.lex();
        self.dispatch_directive(hash_span, token);
    }

    fn dispatch_directive(&mut self, hash_span: Span, token: Token) {
        match token.kind {
            TokenKind::Name(pp_kw::Include) => self.include(token.span),
            TokenKind::Name(pp_kw::Embed) => self.embed(token.span),
            TokenKind::Name(pp_kw::Define) => self.define(token.span),
            TokenKind::Name(pp_kw::Undef) => self.undef(token.span),
            TokenKind::Name(pp_kw::Line) => self.line(token.span),
            TokenKind::Name(pp_kw::Error) => self.error(token.span),
            TokenKind::Name(pp_kw::Warning) => self.warning(token.span),
            TokenKind::Name(pp_kw::Pragma) => self.pragma(hash_span),
            TokenKind::Name(kw::Else) => self.r#else(token.span),
            TokenKind::Name(pp_kw::Endif) => self.endif(token.span),
            TokenKind::Name(n @ (kw::If | pp_kw::Ifdef | pp_kw::Ifndef)) => {
                self.r#if(n, token.span)
            }
            TokenKind::Name(n @ (pp_kw::Elif | pp_kw::Elifdef | pp_kw::Elifndef)) => {
                self.elif(n, token.span)
            }

            _ => {
                self.pp.diags.emit(InvalidDirective {
                    is_name: matches!(token.kind, TokenKind::Name(_)),
                    span: token.span,
                });
            }
        }
    }

    fn r#if(&mut self, directive: Name, span: Span) {
        let value = self.eat_and_eval_expr(directive, span);
        self.cond_stack.push(Cond {
            directive,
            directive_span: span,
            entered: value,
            in_else: false,
        });

        if !value {
            self.skip_inactive_cond_block();
        }
    }

    fn elif(&mut self, directive: Name, span: Span) {
        let Some(cond) = self.cond_stack.last() else {
            self.pp.diags.emit(NoIf { directive, span });
            return;
        };

        if cond.in_else {
            self.pp
                .diags
                .emit(InvalidDirectiveAfterElse { directive, span });
            return;
        }

        if !cond.entered && self.eat_and_eval_expr(directive, span) {
            self.cond_stack.last_mut().unwrap().entered = true;
        } else {
            self.skip_inactive_cond_block();
        }
    }

    fn r#else(&mut self, span: Span) {
        {
            let Some(cond) = self.cond_stack.last() else {
                self.pp.diags.emit(NoIf {
                    directive: kw::Else,
                    span,
                });
                return;
            };

            if cond.in_else {
                self.pp.diags.emit(InvalidDirectiveAfterElse {
                    directive: kw::Else,
                    span,
                });
                return;
            }
        }

        self.check_extra_tokens();

        let Some(cond) = self.cond_stack.last_mut() else {
            return;
        };

        cond.in_else = true;
        if cond.entered {
            self.skip_inactive_cond_block();
        }
    }

    fn endif(&mut self, span: Span) {
        self.check_extra_tokens();

        if self.cond_stack.pop().is_none() {
            self.pp.diags.emit(NoIf {
                directive: pp_kw::Endif,
                span,
            });
        }
    }

    // on dit "skip" car on ignore les tokens complètement contrairement aux fonctions
    //  "eat_*" où on lex vraiment (et donc qui peut produire des erreurs etc)
    fn skip_until_directive(&mut self) {
        loop {
            let at_bol = self.lexer.at_bol();
            // on n'utilise pas `lexer.bump()` pour éviter de décoder les UCN
            // (on veut pas d'erreur par rapport à ça)
            let c = self.lexer.chars.next();
            self.lexer.eat_whitespace();

            // on vérifie juste si le char est '#' même si ça pourrait être le
            // début d'un token '##' (donc ça ne serait pas un token Hash),
            // mais c'est pas grave car la fonction skip_inactive_cond_block nous
            // rappelle en boucle donc on se rendra bien compte que c'était en fait
            // pas une directive
            if c == Some('#') && at_bol || c.is_none() {
                return;
            }
        }
    }

    fn skip_inactive_cond_block(&mut self) {
        let mut nest = 0;
        loop {
            self.skip_until_directive();
            if self.lexer.eof() {
                return;
            }
            if self.lexer.at_bol() {
                // null directive
                continue;
            }
            let token = self.lexer.lex();

            match token.kind {
                TokenKind::Name(kw::If | pp_kw::Ifdef | pp_kw::Ifndef) => nest += 1,
                TokenKind::Name(pp_kw::Endif) if nest == 0 => {
                    self.endif(token.span);
                    return;
                }
                TokenKind::Name(pp_kw::Endif) => nest -= 1,

                TokenKind::Name(pp_kw::Elif | pp_kw::Elifdef | pp_kw::Elifndef | kw::Else)
                    if nest == 0 =>
                {
                    // le span est faux mais on s'en sert pas dans ce cas donc
                    // osef (ça serait chiant de récupérer le vrai, il a été mangé
                    // par skip_until_directive)
                    // todo: c'est dommage que cette fonction ait besoin du hash span,
                    // on pourrait potentiellement faire en sorte de ne pas en avoir
                    // besoin mais c'est le plus simple pour l'instant
                    let hash_span = Span {
                        lo: Loc(0),
                        hi: Loc(0),
                    };
                    self.dispatch_directive(hash_span, token);
                    return;
                }

                _ => {}
            }
        }
    }

    fn eat_until_newline(&mut self) -> Vec<Token> {
        // todo: temp alloc
        let mut tokens = Vec::new();
        while !self.lexer.at_bol() {
            let token = self.lexer.lex();
            match token.kind {
                TokenKind::Eof => break,
                _ => tokens.push(token),
            }
        }

        tokens
    }

    fn eat_if_expr(&mut self) -> Vec<Token> {
        // todo: temp alloc
        let mut tokens = Vec::new();
        while !self.lexer.at_bol() {
            let curr = self.lexer.lex();
            match curr.kind {
                TokenKind::Eof => break,
                TokenKind::Name(pp_kw::HasInclude | pp_kw::HasEmbed) => {
                    // si on reconnait `__has_include(` ou `__has_embed(`, il faut
                    // lexer le token suivant comme un header-name au lieu d'un
                    // token normal
                    tokens.push(curr);

                    if self.lexer.at_bol() {
                        break;
                    }

                    let next = self.lexer.lex();
                    let is_paren_l = next.kind == TokenKind::ParenL;
                    tokens.push(next);

                    if is_paren_l && let Some(header) = self.lexer.lex_header_name() {
                        tokens.push(header);
                    }
                }
                _ => tokens.push(curr),
            }
        }

        tokens
    }

    fn eat_and_eval_expr(&mut self, directive: Name, span: Span) -> bool {
        match directive {
            kw::If | pp_kw::Elif => {
                let mut tokens = self.eat_if_expr();
                tokens = self.eval_defined_exprs(tokens);
                tokens = self.expand_tokens(&tokens, false);
                tokens = self.eval_has_exprs(tokens);

                for t in &mut tokens {
                    match t.kind {
                        TokenKind::Name(pp_kw::Defined) => {
                            self.pp.diags.emit(DefinedAppearedAfterExpansion {
                                span: self.pp.shub.walk_up_to_source(t.span),
                            });
                            return false;
                        }
                        TokenKind::Name(kw::True | kw::False) => {}
                        TokenKind::Name(name) if !self.pp.mac_table.contains_key(&name) => {
                            // normalement les identifiers qui ne sont pas des macros
                            // sont remplacés par `0` mais ça obligerait à écrire le
                            // 0 dans une source virtuelle et pointer dessus avec
                            // une substitution (pour avoir les bonnes locations),
                            // c'est chiant donc on remplace par un token False à
                            // la place, ce qui devrait être équivalent puisque
                            // l'interpréteur traite les bool comme 0 ou 1
                            t.kind = TokenKind::Name(kw::False);
                        }
                        _ => {}
                    }
                }

                if tokens.is_empty() {
                    self.pp
                        .diags
                        .emit(ExpectedTokensInDirective { directive, span });
                    return false;
                }

                match ExprParser::new(&tokens, self.pp.shub, self.pp.diags).parse() {
                    Ok(ops) => Interpreter::new().eval(&ops) != 0,
                    Err(errors) => {
                        self.pp.diags.emit(InvalidExpr(errors, directive));
                        false
                    }
                }
            }

            pp_kw::Ifdef | pp_kw::Ifndef | pp_kw::Elifdef | pp_kw::Elifndef => {
                let tokens = self.eat_until_newline();
                let mut tokens_slice = tokens.as_slice();
                let Some(t) = tokens_slice.split_off_first() else {
                    self.pp
                        .diags
                        .emit(ExpectedTokensInDirective { directive, span });
                    return false;
                };
                let TokenKind::Name(name) = t.kind else {
                    self.pp.diags.emit(InvalidMacName {
                        lexeme: t.lexeme(self.pp.shub).into_owned(),
                        span: t.span,
                        is_name: false,
                    });
                    return false;
                };

                if !tokens_slice.is_empty() {
                    self.pp.diags.emit(TokensAfterDirective {
                        spans: tokens_slice.iter().map(|t| t.span).collect(),
                    })
                }

                let def = matches!(directive, pp_kw::Ifdef | pp_kw::Elifdef);
                self.pp.mac_table.contains_key(&name) == def
            }

            _ => unreachable!(),
        }
    }

    fn eval_defined_exprs(&mut self, tokens: Vec<Token>) -> Vec<Token> {
        if !tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Name(pp_kw::Defined)))
        {
            // rien à faire
            return tokens;
        }

        let mut out = Vec::new();
        // todo: ça serait mieux into_iter mais parse_parens veut un iter sur des
        // &Token
        let mut it = tokens.iter();
        while let Some(curr) = it.next() {
            let curr = curr.clone();
            match curr.kind {
                TokenKind::Name(pp_kw::Defined) => {
                    let token_bool = |b| Token {
                        kind: TokenKind::Name(if b { kw::True } else { kw::False }),
                        ..curr
                    };

                    let Some(next) = it.clone().next() else {
                        self.pp.diags.emit(InvalidDefinedOperand {
                            span: curr.span,
                            has_operand: false,
                            has_parens: false,
                        });
                        out.push(token_bool(false));
                        continue;
                    };

                    #[rustfmt::skip]
                    let name = match next.kind {
                        TokenKind::Name(name) => {
                            it.next();
                            name
                        }
                        TokenKind::ParenL => match parse_parens(&mut it) {
                            Ok(parens) => match parens.tokens.as_slice() {
                                [Token { kind: TokenKind::Name(name), .. }] => *name,
                                _ => {
                                    if let Some(first) = parens.tokens.first()
                                        && let Some(last) = parens.tokens.last()
                                    {
                                        self.pp.diags.emit(InvalidDefinedOperand {
                                            span: self.pp.shub.merge(first.span, last.span),
                                            has_operand: true,
                                            has_parens: true,
                                        });
                                    } else {
                                        self.pp.diags.emit(ExpectedOperandInParens {
                                            operator: pp_kw::Defined,
                                            span: self.pp.shub.merge(parens.l_span, parens.r_span),
                                            has_parens: true,
                                        });
                                    }
                                    out.push(token_bool(false));
                                    continue;
                                }
                            },
                            Err(ParseParensError::NoParenL) => unreachable!(),
                            Err(ParseParensError::NoParenR(span)) => {
                                self.pp.diags.emit(UnmatchedParenL { span });
                                out.push(token_bool(false));
                                continue;
                            }
                        },
                        _ => {
                            // on mange le next même si il est pas bon car dans
                            // tous les cas c'est censé être l'opérande du defined,
                            // on veut pas que ça génère d'autres erreurs qui ont
                            // rien à voir
                            it.next();
                            self.pp.diags.emit(InvalidDefinedOperand {
                                span: next.span,
                                has_operand: true,
                                has_parens: false,
                            });
                            out.push(token_bool(false));
                            continue;
                        }
                    };

                    out.push(token_bool(self.pp.mac_table.contains_key(&name)));
                }
                _ => out.push(curr),
            }
        }

        out
    }

    fn eval_has_exprs(&mut self, tokens: Vec<Token>) -> Vec<Token> {
        let is_has_expr = |name| {
            matches!(
                name,
                pp_kw::HasCppAttribute | pp_kw::HasInclude | pp_kw::HasEmbed
            )
        };

        if !tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Name(name) if is_has_expr(name)))
        {
            // rien à faire
            return tokens;
        }

        let mut out = Vec::new();
        // todo: ça serait mieux d'utiliser into_iter mais parse_parens veut
        // un iterator sur des &Token
        let mut it = tokens.iter();
        while let Some(curr) = it.next() {
            let curr = curr.clone();
            match curr.kind {
                TokenKind::Name(name) if is_has_expr(name) => {
                    let parens = match parse_parens(&mut it) {
                        Ok(x) => x,
                        Err(e) => {
                            match e {
                                ParseParensError::NoParenL => {
                                    self.pp.diags.emit(ExpectedOperandInParens {
                                        operator: name,
                                        span: curr.span,
                                        has_parens: false,
                                    });
                                }
                                ParseParensError::NoParenR(span) => {
                                    self.pp.diags.emit(UnmatchedParenL { span })
                                }
                            }
                            // on met un token faux pour pas avoir en plus l'erreur
                            // "expected expression"
                            out.push(Token {
                                kind: TokenKind::Name(kw::False),
                                ..curr
                            });
                            continue;
                        }
                    };

                    let token = match name {
                        // todo: les fonctions ci-dessous vont générer des tokens
                        // mais elles n'appliquent pas de substitution sur ces tokens
                        // car c'est pas forcément nécessaire vu qu'on est dans
                        // un #if (les tokens sont évalués puis jetés) mais
                        // peut-être qu'il faudrait quand même faire des substs
                        // pour l'affichage des erreurs liées à l'expression
                        // (erreur de parsing)
                        pp_kw::HasCppAttribute => self.eval_has_cpp_attribute(curr, parens),
                        pp_kw::HasInclude => self.eval_has_include(curr, parens),
                        pp_kw::HasEmbed => self.eval_has_embed(curr, parens),
                        _ => continue,
                    };

                    out.push(token);
                }
                _ => out.push(curr),
            }
        }

        out
    }

    fn eval_has_cpp_attribute(&mut self, curr: Token, parens: ParsedParens) -> Token {
        let token_false = Token {
            kind: TokenKind::Name(kw::False),
            ..curr
        };

        #[rustfmt::skip]
        let (namespace, name) = match parens.tokens.as_slice() {
            [Token { kind: TokenKind::Name(name), .. }] => (None, *name),

            [
                Token { kind: TokenKind::Name(namespace), .. },
                Token { kind: TokenKind::ColonColon, .. },
                Token { kind: TokenKind::Name(name), .. },
            ] => (Some(*namespace), *name),

            [] => {
                self.pp.diags.emit(ExpectedOperandInParens {
                    operator: pp_kw::HasCppAttribute,
                    span: self.pp.shub.merge(parens.l_span, parens.r_span),
                    has_parens: true,
                });
                return token_false;
            }
            _ => {
                self.pp.diags.emit(InvalidAttr {
                    span: self.pp.shub.merge(
                        parens.tokens.first().unwrap().span,
                        parens.tokens.last().unwrap().span
                    ),
                });
                return token_false;
            }
        };

        if let Some(attr) = self
            .pp
            .attrs
            .iter()
            .find(|&a| a.namespace == namespace && a.name == name)
        {
            attr.token.clone()
        } else {
            token_false
        }
    }

    fn eval_has_include(&mut self, curr: Token, parens: ParsedParens) -> Token {
        let tokens = parens.tokens;
        let token_bool = |b| Token {
            kind: TokenKind::Name(if b { kw::True } else { kw::False }),
            ..curr
        };

        if tokens.is_empty() {
            self.pp.diags.emit(ExpectedOperandInParens {
                operator: pp_kw::HasInclude,
                span: self.pp.shub.merge(parens.l_span, parens.r_span),
                has_parens: true,
            });
            return token_bool(false);
        }

        let Some((name, kind, consumed)) = self.extract_header_name(&tokens) else {
            return token_bool(false);
        };

        let resolved_header =
            self.pp
                .resolve_header(HeaderGroup::Include, &name, kind, self.source_id);

        let mut invalid_header = |error| {
            let first = tokens.first().unwrap();
            let last = tokens.last().unwrap();
            self.pp.diags.emit(InvalidHeader {
                error,
                span: self.pp.shub.merge(first.span, last.span),
            });
        };

        if consumed != tokens.len() {
            invalid_header(HeaderError::Malformed);
            return token_bool(false);
        }

        let Some((path, status)) = resolved_header else {
            return token_bool(false);
        };

        if status == FileStatus::Available
            && str::from_utf8(&self.pp.file_loader.load(&path).unwrap()).is_ok()
        {
            token_bool(true)
        } else {
            invalid_header(HeaderError::Unreadable(name));
            token_bool(false)
        }
    }

    fn eval_has_embed(&mut self, curr: Token, parens: ParsedParens) -> Token {
        const EMBED_NOT_FOUND: &str = "0";
        const EMBED_FOUND: &str = "1";
        const EMBED_EMPTY: &str = "2";

        let tokens = parens.tokens;
        let token_false = Token {
            kind: TokenKind::Name(kw::False),
            ..curr
        };

        if tokens.is_empty() {
            self.pp.diags.emit(ExpectedOperandInParens {
                operator: pp_kw::HasEmbed,
                span: self.pp.shub.merge(parens.l_span, parens.r_span),
                has_parens: true,
            });
            return token_false;
        }

        let Some((name, kind, consumed)) = self.extract_header_name(&tokens) else {
            return token_false;
        };

        let params = parse_embed_params(&tokens[consumed..], self.pp.shub, self.pp.diags);
        let (params, saw_unknowns_params) = extract_embed_params(params, false, self.pp);

        let resolved_header =
            self.pp
                .resolve_header(HeaderGroup::Embed, &name, kind, self.source_id);

        let mut token = |embed: &str| {
            let span = to_span(
                0..embed.len() as u32,
                self.pp.shub.write_virtual_source(embed),
            );
            Token::new(TokenKind::Number, span, curr.space_before)
        };

        let Some((path, status)) = resolved_header else {
            return token(EMBED_NOT_FOUND);
        };

        if status != FileStatus::Available {
            let first = &tokens[0];
            let last = &tokens[consumed - 1];
            self.pp.diags.emit(InvalidHeader {
                error: HeaderError::Unreadable(name),
                span: self.pp.shub.merge(first.span, last.span),
            });
            return token_false;
        }

        token(if saw_unknowns_params {
            EMBED_NOT_FOUND
        } else {
            // todo: pas besoin de charger complètement le fichier, on veut juste
            // connaître sa taille
            let data = self.pp.file_loader.load(&path).unwrap();
            let count = data.len().min(params.limit.unwrap_or(usize::MAX));
            if count == 0 { EMBED_EMPTY } else { EMBED_FOUND }
        })
    }

    /// forme un header name à partir des tokens donnés, on suppose qu'ils ont
    /// été expandés au préalable et que le header à extraire se trouve au début
    /// des tokens
    ///
    /// retourne le nom du header, le kind et le nombre de tokens consommés et
    /// None en cas d'erreur
    fn extract_header_name(&mut self, tokens: &[Token]) -> Option<(String, HeaderKind, usize)> {
        let mut it = tokens.iter();
        let first = it.next()?;
        let mut consumed = 1;

        let mut invalid_header = |error| {
            self.pp.diags.emit(InvalidHeader {
                error,
                span: self.pp.shub.merge(first.span, tokens.last().unwrap().span),
            });
        };

        let (mut name, kind) = match first.kind {
            TokenKind::Header(kind) => (first.lexeme(self.pp.shub).into_owned(), kind),
            TokenKind::Str(StrKind::NonRaw, Encoding::Ordinary, _, None) => {
                (first.lexeme(self.pp.shub).into_owned(), HeaderKind::Quote)
            }
            TokenKind::Lt => {
                loop {
                    let next = it.next();
                    if next.is_none() {
                        invalid_header(HeaderError::Malformed);
                        return None;
                    }
                    if next.is_some_and(|t| t.kind == TokenKind::Gt) {
                        break;
                    }
                }

                consumed = tokens.len() - it.as_slice().len();
                let name = format_angle_header_name(&tokens[..consumed], self.pp.shub);
                (name, HeaderKind::Angle)
            }
            _ => {
                invalid_header(HeaderError::Malformed);
                return None;
            }
        };

        // ignore les angles brackets / quotes
        name.remove(0);
        name.pop();

        if name.is_empty() {
            invalid_header(HeaderError::Empty);
            return None;
        }

        Some((name, kind, consumed))
    }

    /// évalue les opérateurs _Pragma présents dans self.tokens à partir de start,
    /// en les réinjectant dans le flux de tokens si le préprocesseur ne sait
    /// pas les gérer
    fn handle_pragma_ops(&mut self, start: usize) {
        struct Pragma {
            tokens: Vec<Token>,
            range: Range<usize>,
            pragma_token: Token,
            paren_r: Token,
            body_span: Span,
        }

        // on extrait les _Pragma pour les évaluer par la suite
        // todo: temp alloc
        let mut pragmas = Vec::new();
        let tokens = &self.tokens[start..];
        let mut pending_tokens = VecDeque::new();
        let mut it = tokens.iter();
        let mut i = start;
        'outer: while let Some(curr) = it.next() {
            if curr.kind == TokenKind::Name(pp_kw::PragmaOp) {
                let pragma_start = i;
                // on utilise le lexer si besoin car il se peut que l'argument du _Pragma
                // (ou même les parenthèses) ne soient pas dans les tokens mais dans la suite
                // du code (pas encore lexé)
                // du coup il faut avancer dans le lexer sans oublier d'expand
                // si besoin les nouveaux tokens
                //
                // on pourrait éviter de devoir faire ça si le mac expander
                // considérait le _Pragma un peu comme une fn-like macro, en
                // expandant tout jusqu'à la parenthèse fermante mais ça a l'air
                // chiant à faire donc il recrache juste le _Pragma et c'est à nous
                // de le gérer
                let at_bol = Cell::new(false);
                let mut get_next = || {
                    i += 1;
                    it.next()
                        .cloned()
                        .or_else(|| pending_tokens.pop_front())
                        .unwrap_or_else(|| {
                            loop {
                                let token = self.lexer.lex();
                                at_bol.set(self.lexer.at_bol());

                                if matches!(token.kind, TokenKind::Name(_)) {
                                    let mut expander = MacExpander::new(self.pp, true);
                                    let mut expanded = Vec::new();
                                    let mut next_needs_space = false;
                                    expander.expand(
                                        &mut expanded,
                                        token,
                                        Some(&mut self.lexer),
                                        &mut next_needs_space,
                                    );
                                    pending_tokens.extend(expanded);
                                } else {
                                    pending_tokens.push_back(token);
                                }

                                if let Some(t) = pending_tokens.pop_front() {
                                    return t;
                                }
                            }
                        })
                };

                let paren_l = get_next();
                if paren_l.kind != TokenKind::ParenL {
                    self.pp.diags.emit(ExpectedOperandInParens {
                        operator: pp_kw::PragmaOp,
                        span: curr.span,
                        has_parens: false,
                    });
                    continue;
                }

                let is_str = |token: &Token| {
                    matches!(token.kind, TokenKind::Str(StrKind::NonRaw, _, _, None))
                };
                // todo: temp alloc
                let mut operand_tokens = Vec::new();
                let paren_r;
                loop {
                    let curr = get_next();
                    match curr.kind {
                        TokenKind::Eof => {
                            self.pp.diags.emit(UnmatchedParenL { span: paren_l.span });
                            return;
                        }
                        TokenKind::ParenR => {
                            paren_r = curr;
                            break;
                        }
                        _ => {
                            let str = is_str(&curr);
                            operand_tokens.push(curr);

                            if !str && at_bol.get() {
                                self.pp.diags.emit(InvalidPragmaOperand {
                                    span: operand_tokens.first().unwrap().span,
                                });
                                continue 'outer;
                            }
                        }
                    }
                }

                let pragma_end = (i + 1).min(self.tokens.len());
                let Some(operand) = operand_tokens.first() else {
                    self.pp.diags.emit(ExpectedOperandInParens {
                        operator: pp_kw::PragmaOp,
                        span: self.pp.shub.merge(paren_l.span, paren_r.span),
                        has_parens: true,
                    });
                    continue;
                };

                if !is_str(operand) || operand_tokens.len() != 1 {
                    let first = operand_tokens.first().unwrap();
                    let last = operand_tokens.last().unwrap();
                    self.pp.diags.emit(InvalidPragmaOperand {
                        span: self.pp.shub.merge(first.span, last.span),
                    });
                    continue;
                }

                let lexeme = operand.lexeme(self.pp.shub).into_owned();
                // on enlève le préfixe et les quotes
                let lexeme = &lexeme[lexeme.find('"').unwrap() + 1..lexeme.len() - 1];
                let unescaped = if lexeme.contains('\\') {
                    let chars = lexeme.chars();
                    &UnescapePragma { chars }.collect::<String>()
                } else {
                    lexeme
                };

                let source_start = self.pp.shub.write_virtual_source(unescaped);
                let mut lexer = Lexer::new(unescaped, source_start);
                let tokens = lexer.lex_until_eof();

                pragmas.push(Pragma {
                    range: pragma_start..pragma_end,
                    tokens,
                    pragma_token: curr.clone(),
                    paren_r,
                    body_span: Span {
                        lo: source_start,
                        hi: Loc(source_start.0 + unescaped.len() as u32),
                    },
                });
            }

            i += 1;
        }

        // on itère à l'envers car le range des pragmas correspond à l'indice
        // dans self.tokens mais si on splice en avançant de gauche à droite ça
        // rend les indices invalides
        for mut p in pragmas.into_iter().rev() {
            if !self.handle_pragma(&p.tokens) {
                // on réécrit les tokens tels quels, en les entourant d'un token
                // BuiltinPragma et BuiltinPragmaEnd pour les gérer dans la suite
                // de la compilation
                // on fait une substitution car les tokens ne viennent pas de
                // la source (ils ont été générés à partir de la string du _Pragma)
                let subst = self.pp.shub.add_subst(
                    SubstKind::Other,
                    self.pp.shub.merge(p.pragma_token.span, p.paren_r.span),
                    p.body_span,
                );

                for t in &mut p.tokens {
                    t.span = t.span.apply_subst(subst);
                }

                let start = Token {
                    kind: TokenKind::Name(pp_kw::BuiltinPragma),
                    space_before: false,
                    ..p.pragma_token
                };
                let end = Token {
                    kind: TokenKind::Name(pp_kw::BuiltinPragmaEnd),
                    space_before: false,
                    ..p.paren_r
                };
                p.tokens.insert(0, start);
                p.tokens.push(end);
                self.tokens.splice(p.range, p.tokens);
            }
        }
    }

    fn undef(&mut self, span: Span) {
        if self.lexer.at_bol() {
            self.pp.diags.emit(ExpectedTokensInDirective {
                directive: pp_kw::Undef,
                span,
            });
            return;
        }

        let token = self.lexer.lex();
        let TokenKind::Name(name) = token.kind else {
            self.pp.diags.emit(InvalidMacName {
                lexeme: token.lexeme(self.pp.shub).into_owned(),
                span: token.span,
                is_name: false,
            });
            return;
        };

        self.check_mac_name(name, token.span, None);

        match self.pp.mac_table.entry(name) {
            Entry::Occupied(e) => {
                if e.get().predefined {
                    self.pp.diags.emit(RedefinedPredefMac {
                        name,
                        span: token.span,
                        is_define: false,
                    });
                } else {
                    e.remove();
                }
            }
            Entry::Vacant(_) => {}
        }

        self.check_extra_tokens();
    }

    fn error(&mut self, span: Span) {
        let tokens = self.eat_until_newline();
        self.pp.diags.emit(ErrorWarningDirective {
            is_warn: false,
            span,
            message: format_pp_output(&tokens, self.pp.shub),
        });
    }

    fn warning(&mut self, span: Span) {
        let tokens = self.eat_until_newline();
        self.pp.diags.emit(ErrorWarningDirective {
            is_warn: true,
            span,
            message: format_pp_output(&tokens, self.pp.shub),
        });
    }

    fn pragma(&mut self, hash_span: Span) {
        let tokens = self.eat_until_newline();
        if !self.handle_pragma(&tokens) {
            // on réécrit les tokens tels quels, en les entourant d'un token BuiltinPragma
            // et BuiltinPragmaEnd pour les gérer dans la suite de la compilation
            //
            // pour le range de ces tokens builtin, on leur donne le range du `#`
            // ce qui est un énorme mensonge mais on s'en fout
            //
            // si on voulait avoir le bon range il faudrait écrire le lexème de ces
            // tokens dans une source virtuelle et créer une substitution qui pointe
            // dessus, c'est assez chiant pour rien
            // on peut pas juste lui donner un range de 0..0 (ça aurait donc un lexème
            // vide) car il faut que ça soit placé sur la bonne ligne sinon il y aurait
            // pas le bon nombre de newlines dans la sortie du préprocesseur
            self.tokens.push(Token::new(
                TokenKind::Name(pp_kw::BuiltinPragma),
                hash_span,
                false,
            ));
            self.tokens.extend(tokens);
            self.tokens.push(Token::new(
                TokenKind::Name(pp_kw::BuiltinPragmaEnd),
                hash_span,
                false,
            ));
        }
    }

    fn include(&mut self, span: Span) {
        // todo: temp alloc
        let mut tokens = Vec::new();
        if let Some(header) = self.lexer.lex_header_name() {
            tokens.push(header);
        }
        tokens.extend(self.eat_until_newline());
        tokens = self.expand_tokens(&tokens, true);

        if tokens.is_empty() {
            self.pp.diags.emit(ExpectedTokensInDirective {
                directive: pp_kw::Include,
                span,
            });
            return;
        }

        let Some((name, kind, consumed)) = self.extract_header_name(&tokens) else {
            return;
        };

        if consumed != tokens.len() {
            self.pp.diags.emit(TokensAfterDirective {
                spans: tokens[consumed..].iter().map(|t| t.span).collect(),
            });
        }

        let resolved_header =
            self.pp
                .resolve_header(HeaderGroup::Include, &name, kind, self.source_id);

        let mut invalid_header = |error| {
            let first = &tokens[0];
            let last = &tokens[consumed - 1];
            self.pp.diags.emit(InvalidHeader {
                error,
                span: self.pp.shub.merge(first.span, last.span),
            });
        };

        let Some((path, status)) = resolved_header else {
            invalid_header(HeaderError::NotFound(name));
            return;
        };

        if self.pp.pragma_once_paths.contains(&path) {
            return;
        }

        // todo: il faudrait peut-être que resolve_header retourne directement
        // le texte sinon on risque un TOCTOU
        if status == FileStatus::Available
            && let Ok(text) = String::from_utf8(self.pp.file_loader.load(&path).unwrap())
        {
            if self.include_depth > self.pp.opts.max_include_depth {
                self.pp.diags.emit(ExceededMaxIncludeDepth {
                    max: self.pp.opts.max_include_depth,
                    span,
                });
                return;
            }

            let text_len = text.len();
            let source = self.pp.shub.add_source(path, text);
            let source_id = source.id();
            let source_start = source.start();
            let subst = {
                let dst = Span {
                    lo: source_start,
                    hi: Loc(source_start.0 + text_len as u32),
                };
                self.pp.shub.add_subst(SubstKind::Include, span, dst)
            };

            // on applique la subst directement sur le source_start pour qu'elle
            // soit appliquée en premier (avant les éventuelles autres substs
            // qui se produiront lors du preprocessing)
            let start = source_start.apply_subst(subst);
            let depth = self.include_depth + 1;
            InnerPreprocessor::new(self.pp, source_id, start, depth, self.tokens).preprocess();
        } else {
            invalid_header(HeaderError::Unreadable(name));
        }
    }

    fn embed(&mut self, span: Span) {
        let mut tokens = Vec::new();
        if let Some(header) = self.lexer.lex_header_name() {
            tokens.push(header);
        }
        tokens.extend(self.eat_until_newline());
        tokens = self.expand_tokens(&tokens, true);

        if tokens.is_empty() {
            self.pp.diags.emit(ExpectedTokensInDirective {
                directive: pp_kw::Embed,
                span,
            });
            return;
        }

        let Some((name, kind, consumed)) = self.extract_header_name(&tokens) else {
            return;
        };

        let params = parse_embed_params(&tokens[consumed..], self.pp.shub, self.pp.diags);
        let (params, _) = extract_embed_params(params, true, self.pp);

        let resolved_header =
            self.pp
                .resolve_header(HeaderGroup::Embed, &name, kind, self.source_id);

        let mut invalid_header = |error| {
            let first = &tokens[0];
            let last = &tokens[consumed - 1];
            self.pp.diags.emit(InvalidHeader {
                error,
                span: self.pp.shub.merge(first.span, last.span),
            });
        };

        let Some((path, status)) = resolved_header else {
            invalid_header(HeaderError::NotFound(name));
            return;
        };

        if status != FileStatus::Available {
            invalid_header(HeaderError::Unreadable(name));
            return;
        }

        let data = self.pp.file_loader.load(&path).unwrap();
        let count = data.len().min(params.limit.unwrap_or(usize::MAX));

        let tokens = if count == 0 {
            if let Some(tokens) = params.if_empty {
                tokens
            } else {
                // rien à faire
                return;
            }
        } else {
            // on crée le texte contenant la liste des bytes séparés par une
            // virgule, puis on le lex
            let mut text = String::with_capacity(count * 3);
            write!(&mut text, "{}", data[0]).unwrap();
            for byte in &data[1..count] {
                write!(&mut text, ",{byte}").unwrap();
            }

            let source_start = self.pp.shub.write_virtual_source(&text);
            let subst = {
                let dst = Span {
                    lo: source_start,
                    hi: Loc(source_start.0 + text.len() as u32),
                };
                self.pp.shub.add_subst(SubstKind::Other, span, dst)
            };

            // ça serait peut-être plus rapide de créer les tokens à la main
            // (c'est juste des ints et virgules) mais bon
            Lexer::new(&text, source_start.apply_subst(subst)).lex_until_eof()
        };

        self.tokens.extend(params.prefix.unwrap_or_default());
        self.tokens.extend(tokens);
        self.tokens.extend(params.suffix.unwrap_or_default());
    }

    fn line(&mut self, span: Span) {
        let raw_tokens = self.eat_until_newline();
        let mut tokens = self.expand_tokens(&raw_tokens, true);

        let Some(t) = tokens.try_remove(0) else {
            self.pp.diags.emit(ExpectedTokensInDirective {
                directive: pp_kw::Line,
                span,
            });
            return;
        };

        let line = match t.kind {
            TokenKind::Number => {
                let lexeme = t.lexeme(self.pp.shub);
                let is_oct = {
                    let is_hex = lexeme.starts_with("0x");
                    let is_bin = lexeme.starts_with("0b");
                    !is_hex && !is_bin && lexeme.starts_with('0') && lexeme != "0"
                };
                let lexeme = if is_oct {
                    lexeme.trim_start_matches('0')
                } else {
                    &lexeme
                };

                match parse_number(lexeme) {
                    Ok(NumberLit {
                        kind: NumberLitKind::Int { value, .. },
                        ..
                    }) => {
                        if lexeme.contains(|c| !matches!(c, '0'..='9' | '\'')) {
                            self.pp.diags.emit(InvalidLineNumber {
                                kind: InvalidLineNumberKind::InvalidDigits,
                                span: t.span,
                            });
                            return;
                        }
                        if is_oct {
                            self.pp.diags.emit(OctalNumberInLineDirective {
                                span: t.span,
                                value,
                            });
                        }
                        if !(1..=i32::MAX as i128).contains(&value) {
                            self.pp.diags.emit(InvalidLineNumber {
                                kind: InvalidLineNumberKind::OutOfRange,
                                span: t.span,
                            });
                            return;
                        }
                        value as u32
                    }
                    Err(ParseNumberError::IntValueTooLarge) => {
                        self.pp.diags.emit(InvalidLineNumber {
                            kind: InvalidLineNumberKind::OutOfRange,
                            span: t.span,
                        });
                        return;
                    }
                    _ => {
                        self.pp.diags.emit(InvalidLineNumber {
                            kind: InvalidLineNumberKind::NotANumber,
                            span: t.span,
                        });
                        return;
                    }
                }
            }
            _ => {
                self.pp.diags.emit(InvalidLineNumber {
                    kind: InvalidLineNumberKind::NotANumber,
                    span: t.span,
                });
                return;
            }
        };

        let file_name = tokens.try_remove(0).and_then(|t| {
            if let TokenKind::Str(StrKind::NonRaw, Encoding::Ordinary, mut value, None) = t.kind {
                // pop le 0 terminal
                value.pop();
                Some(value)
            } else {
                self.pp.diags.emit(InvalidLineFileName { span: t.span });
                None
            }
        });

        if !tokens.is_empty() {
            self.pp.diags.emit(TokensAfterDirective {
                spans: tokens.iter().map(|t| t.span).collect(),
            });
        }

        self.pp.shub.add_line_remap(
            self.source_id,
            LineRemap {
                offset: raw_tokens.last().unwrap().span.hi,
                line,
                file_name,
            },
        );
    }

    fn define(&mut self, span: Span) {
        if self.lexer.at_bol() {
            self.pp.diags.emit(ExpectedTokensInDirective {
                directive: pp_kw::Define,
                span,
            });
            return;
        }

        let token = self.lexer.lex();
        let span = token.span;
        let TokenKind::Name(name) = token.kind else {
            self.pp.diags.emit(InvalidMacName {
                lexeme: self.pp.shub.text(span).to_owned(),
                span,
                is_name: false,
            });
            return;
        };

        let mac = if self.lexer.peek(0) == '(' && !self.lexer.next_has_space_before() {
            // c'est une fn-like macro
            self.lexer.lex(); // eat '('

            let mut variadic = false;
            let mut params = Vec::new();
            let mut errors = Vec::new();
            let mut is_first = true;

            loop {
                if self.lexer.at_bol() {
                    errors.push(MacParamListError::HasNewline(span));
                    break;
                }

                let curr = self.lexer.lex();
                match curr.kind {
                    TokenKind::Name(name) => {
                        if find_param(&params, name).is_some() {
                            errors.push(MacParamListError::DuplicateParam(name, curr.span));
                        }
                        params.push(MacParam {
                            name,
                            requires_expansion: false,
                        });
                    }
                    TokenKind::DotDotDot => {
                        if self.lexer.lex().kind != TokenKind::ParenR {
                            errors.push(MacParamListError::EllipsisNotAtEnd(curr.span));
                        }
                        variadic = true;
                        break;
                    }
                    TokenKind::ParenR if is_first => break,
                    _ => {
                        errors.push(MacParamListError::ExpectedName(curr.span));
                        break;
                    }
                }

                if self.lexer.at_bol() {
                    errors.push(MacParamListError::HasNewline(span));
                    break;
                }

                let next = self.lexer.lex();
                match next.kind {
                    TokenKind::Comma => {}
                    TokenKind::ParenR => break,
                    _ => {
                        errors.push(MacParamListError::ExpectedComma(next.span));
                        break;
                    }
                }

                is_first = false;
            }

            if !errors.is_empty() {
                self.pp.diags.emit(InvalidMacParamList(errors));
            }

            if variadic {
                // le dernier param est considéré comme les va args
                params.push(MacParam {
                    name: pp_kw::VaArgs,
                    requires_expansion: false,
                });
            }

            let body = parse_mac_body(variadic, &mut self.lexer, self.pp.diags);
            if let Some(body) = &body {
                let mut it = body.tokens.iter();
                let mut prev = body.tokens.first().expect("il doit y avoir des tokens");
                let mut va_opt_span = None;
                let mut nest = 0;
                while let Some(curr) = it.next() {
                    match curr.kind {
                        TokenKind::Hash => {
                            let is_param = |name| {
                                find_param(&params, name).is_some()
                                    || matches!(name, pp_kw::VaArgs | pp_kw::VaOpt)
                            };

                            if !matches!(
                                it.clone().next(),
                                Some(Token { kind: TokenKind::Name(name), ..}) if is_param(*name)
                            ) {
                                self.pp
                                    .diags
                                    .emit(HashNotFollowedByParam { span: curr.span });
                            }
                        }

                        TokenKind::ParenL if va_opt_span.is_some() => nest += 1,
                        TokenKind::ParenR if va_opt_span.is_some() && nest == 0 => {
                            if prev.kind == TokenKind::HashHash {
                                self.pp.diags.emit(HashHashAtStartOrEnd {
                                    span: prev.span,
                                    at_start: false,
                                    in_va_opt: true,
                                });
                            }
                            va_opt_span = None;
                        }
                        TokenKind::ParenR if va_opt_span.is_some() => nest -= 1,

                        TokenKind::Name(pp_kw::VaOpt) => {
                            if va_opt_span.is_some() {
                                self.pp.diags.emit(NestedVaOpt { span: curr.span });
                                continue;
                            }

                            if let Some(va_arg) = params.last_mut() {
                                va_arg.requires_expansion = true;
                            }

                            let paren_l = it.next();
                            if !paren_l.is_some_and(|t| t.kind == TokenKind::ParenL) && variadic {
                                self.pp.diags.emit(ExpectedOperandInParens {
                                    operator: pp_kw::VaOpt,
                                    span: curr.span,
                                    has_parens: false,
                                });
                                continue;
                            }

                            va_opt_span = paren_l.map(|t| t.span);

                            if let Some(next) = it.clone().next()
                                && next.kind == TokenKind::HashHash
                            {
                                self.pp.diags.emit(HashHashAtStartOrEnd {
                                    span: next.span,
                                    at_start: true,
                                    in_va_opt: true,
                                });
                            }
                        }

                        TokenKind::Name(name) => {
                            let next_is_hash_hash = it
                                .clone()
                                .next()
                                .is_some_and(|t| t.kind == TokenKind::HashHash);

                            let requires_expansion =
                                !matches!(prev.kind, TokenKind::Hash | TokenKind::HashHash)
                                    && !next_is_hash_hash;

                            if let Some(param) = params.iter_mut().find(|p| p.name == name) {
                                param.requires_expansion |= requires_expansion;
                            }
                        }

                        _ => {}
                    }

                    prev = curr;
                }

                if variadic && let Some(span) = va_opt_span {
                    self.pp.diags.emit(UnmatchedParenL { span });
                }
            }

            Mac {
                kind: MacKind::Fn { params, variadic },
                body,
                name_span: Some(span),
                predefined: false,
                expanding: false,
            }
        } else {
            let body = parse_mac_body(false, &mut self.lexer, self.pp.diags);

            if let Some(body) = &body
                && let Some(first) = body.tokens.first()
                && !first.space_before
            {
                self.pp.diags.emit(NoSpaceAfterMacName {
                    name,
                    first_span: first.span,
                });
            }

            Mac {
                kind: MacKind::Obj,
                body,
                name_span: Some(span),
                predefined: false,
                expanding: false,
            }
        };

        self.check_mac_name(name, span, Some(&mac.kind));

        match self.pp.mac_table.entry(name) {
            Entry::Occupied(e) => {
                let old = e.get();
                if old.predefined {
                    self.pp.diags.emit(RedefinedPredefMac {
                        name,
                        span,
                        is_define: true,
                    });
                } else if !is_valid_redefinition(old, &mac, self.pp.shub) {
                    self.pp.diags.emit(MacRedefined {
                        name,
                        old: old.name_span,
                        new: span,
                    });
                }
            }
            Entry::Vacant(e) => {
                e.insert(mac);
            }
        }
    }

    fn check_extra_tokens(&mut self) {
        let tokens = self.eat_until_newline();
        if !tokens.is_empty() {
            self.pp.diags.emit(TokensAfterDirective {
                spans: tokens.into_iter().map(|t| t.span).collect(),
            });
        }
    }

    fn check_mac_name(&mut self, name: Name, span: Span, kind: Option<&MacKind>) {
        let invalid = match name {
            pp_kw::Defined => true,
            attr_kw::Likely | attr_kw::Unlikely => kind == Some(&MacKind::Obj),
            _ => name.is_kw() || name.is_ctxt_kw() || name.is_attr_kw(),
        };

        if invalid {
            self.pp.diags.emit(InvalidMacName {
                lexeme: name.as_str().to_owned(),
                span,
                is_name: true,
            });
        }
    }

    fn expand_tokens(&mut self, tokens: &[Token], forbid_has_exprs: bool) -> Vec<Token> {
        let mut expander = MacExpander::new(self.pp, forbid_has_exprs);

        for t in tokens.iter().rev() {
            expander.exp_stack.push_front(t.clone());
        }

        let mut expanded = Vec::new();
        let mut next_needs_space = false;
        while let Some(next) = expander.exp_stack.pop_front() {
            // normalement il faut utiliser next_needs_space pour forcer l'espace
            // sur le token si besoin, mais ça sert à rien ici parce que chaque
            // fois qu'on appelle cette fonction c'est pour expand des tokens
            // qui ne seront pas gardés donc osef
            expander.expand(&mut expanded, next, None, &mut next_needs_space);
        }

        expanded
    }

    /// retourne vrai si le pragma a été géré (donc il ne doit pas être réinjecté
    /// dans les tokens)
    fn handle_pragma(&mut self, tokens: &[Token]) -> bool {
        if let Some(first) = tokens.first()
            && first.kind == TokenKind::Name(pp_kw::Once)
            && self.pp.opts.pragma_once
        {
            if tokens.len() > 1 {
                self.pp.diags.emit(TokensAfterDirective {
                    spans: tokens[1..].iter().map(|t| t.span).collect(),
                });
            }

            // todo: mieux établir l'identité du fichier
            let path = self.pp.shub.source(self.source_id).path().to_owned();
            self.pp.pragma_once_paths.insert(path);
            return true;
        }

        false
    }
}

fn parse_mac_body(variadic: bool, lexer: &mut Lexer, diags: &mut Diags) -> Option<MacBody> {
    let mut tokens = Vec::new();
    let mut has_concats = false;

    if variadic {
        lexer.forbid_va_args = false;
    }
    while !lexer.at_bol() {
        let token = lexer.lex();
        match token.kind {
            TokenKind::Eof => break,
            TokenKind::HashHash => has_concats = true,
            TokenKind::Name(
                name @ (pp_kw::HasInclude | pp_kw::HasEmbed | pp_kw::HasCppAttribute),
            ) => {
                diags.emit(ForbiddenHasExpr {
                    name,
                    span: token.span,
                });
            }
            _ => {}
        }

        tokens.push(token);
    }
    lexer.forbid_va_args = true;

    if let Some(first) = tokens.first()
        && let Some(last) = tokens.last()
    {
        if first.kind == TokenKind::HashHash {
            diags.emit(HashHashAtStartOrEnd {
                span: first.span,
                at_start: true,
                in_va_opt: false,
            });
        }
        if last.kind == TokenKind::HashHash && first.span != last.span {
            diags.emit(HashHashAtStartOrEnd {
                span: last.span,
                at_start: false,
                in_va_opt: false,
            });
        }

        Some(MacBody {
            span: Span {
                lo: first.span.lo,
                hi: last.span.hi,
            },
            tokens,
            has_concats,
        })
    } else {
        None
    }
}

fn find_param(params: &[MacParam], name: Name) -> Option<usize> {
    params.iter().position(|p| p.name == name)
}

struct MacExpander<'a, 'b> {
    pp: &'a mut Preprocessor<'b>,
    /// stack d'expansion, où on met les tokens en attente de rescan
    exp_stack: VecDeque<Token>,
    /// macros en cours d'expansion, le u32 est l'indice dans la stack d'expansion
    /// à partir duquel la macro ne doit plus être en cours d'expansion
    currently_expanding: Vec<(Name, u32)>,
    forbid_has_exprs: bool,
    saw_pragma: bool,
    expanding_args: bool,
}

impl<'a, 'b> MacExpander<'a, 'b> {
    fn new(pp: &'a mut Preprocessor<'b>, forbid_has_exprs: bool) -> Self {
        Self {
            pp,
            exp_stack: VecDeque::new(),
            currently_expanding: Vec::new(),
            forbid_has_exprs,
            saw_pragma: false,
            expanding_args: false,
        }
    }

    // todo: cette fonction expand un token à la fois mais ça serait probablement
    // mieux qu'elle boucle sur les tokens à expand, pour pas avoir besoin de la
    // rappeler pour chaque token lors du rescan
    fn expand(
        &mut self,
        out: &mut Vec<Token>,
        token: Token,
        mut lexer: Option<&mut Lexer>,
        next_needs_space: &mut bool,
    ) {
        if let TokenKind::Name(name) = token.kind
            && let Some(mac) = self.pp.mac_table.get(&name)
            && !token.frozen
        {
            let name_span = mac.name_span;
            match &mac.kind {
                MacKind::Obj => {
                    let Some(body) = &mac.body else {
                        *next_needs_space = token.space_before;
                        return;
                    };

                    let subst = self.pp.shub.add_subst(
                        SubstKind::MacExpansion { name, name_span },
                        token.span,
                        body.span,
                    );

                    let mut body_tokens = body.tokens.clone();
                    for t in &mut body_tokens {
                        t.span = t.span.apply_subst(subst);
                    }

                    if let Some(first) = body_tokens.first_mut() {
                        first.space_before = token.space_before;
                    }

                    self.concat_and_rescan(
                        body_tokens,
                        token,
                        name,
                        body.has_concats,
                        out,
                        lexer,
                        next_needs_space,
                    );
                }

                MacKind::Fn { params, variadic } => {
                    let mut body = mac.body.clone();
                    // - 1 car on compte pas la parenthèse ouvrante qui va se faire
                    // manger lors du parse des args (on veut la len au début des args)
                    let prev_stack_len = self.exp_stack.len() as i32 - 1;
                    // todo: ça sert strictement à rien de cloner mais sinon le
                    // borrow checker est pas content
                    let params = params.clone();

                    let Some((mut args, paren_r_span)) = self.parse_args(
                        &token,
                        name,
                        name_span,
                        &params,
                        *variadic,
                        out,
                        lexer.as_deref_mut(),
                    ) else {
                        return;
                    };

                    let Some(body) = &mut body else {
                        *next_needs_space = token.space_before;
                        return;
                    };

                    let expanded_at = self.pp.shub.merge(token.span, paren_r_span);
                    let subst = self.pp.shub.add_subst(
                        SubstKind::MacExpansion { name, name_span },
                        expanded_at,
                        body.span,
                    );
                    for t in &mut body.tokens {
                        t.span = t.span.apply_subst(subst);
                    }

                    if let Some(first) = body.tokens.first_mut() {
                        first.space_before = token.space_before;
                    }

                    self.expand_args(
                        &params,
                        &mut args,
                        prev_stack_len,
                        body.tokens.first().unwrap().span,
                    );

                    let tokens =
                        subst_args(&body.tokens, &params, &args, self.pp.shub, self.pp.diags);
                    self.concat_and_rescan(
                        tokens,
                        token,
                        name,
                        body.has_concats,
                        out,
                        lexer,
                        next_needs_space,
                    );
                }

                MacKind::Builtin(builtin) => {
                    let mut expanded = match builtin {
                        BuiltinMac::Line => {
                            // en temps normal on remonte les substitutions jusqu'à
                            // la source pour trouver le point où __LINE__ a été
                            // expandé (pas l'endroit où il apparaît dans le code)
                            // par ex:
                            // ```
                            // #define A __LINE__
                            // A
                            // ```
                            // A doit être remplacé par `2`, même si __LINE__ apparaît
                            // sur la ligne 1
                            // mais si __LINE__ apparaît dans un argument (et n'est
                            // pas imbriqué dans une macro) alors on veut la position
                            // du __LINE__ lui-même et dans ce cas on ne peut pas
                            // juste remonter les substitutions car les arguments
                            // sont considérés comme faisant partie de l'expansion
                            // en cours *avant* d'être eux-même expandés, et donc
                            // dans ce cas on remonterait à l'invocation en cours
                            // mais c'est pas ce qu'on veut, par ex:
                            // ```
                            // #define F(x) x
                            // F(
                            //   __LINE__
                            // )
                            // ```
                            // ici __LINE__ est sur la ligne 3 (et c'est la bonne
                            // réponse) mais si on remontait on tomberait sur le
                            // `F` à la ligne 2
                            //
                            // on regarde si il a une depth de 2 car il faut compter
                            // 1 niveau pour l'expansion de la macro en cours et
                            // 1 niveau pour la subst de l'argument (le __LINE__
                            // dans `F` a bien une depth de 2 au moment de se faire
                            // expand)
                            let is_top_level_arg =
                                self.expanding_args && self.pp.shub.depth(token.span.lo) == 2;
                            let span = if is_top_level_arg {
                                self.pp.shub.source_span(token.span)
                            } else {
                                self.pp.shub.walk_up_to_source(token.span)
                            };
                            let line = self.pp.shub.presumed_full_loc(span.hi).line.to_string();
                            let span = to_span(
                                0..line.len() as u32,
                                self.pp.shub.write_virtual_source(&line),
                            );
                            Token::new(TokenKind::Number, span, token.space_before)
                        }
                        BuiltinMac::File | BuiltinMac::FileName | BuiltinMac::BaseFile => {
                            let file_name = if *builtin == BuiltinMac::BaseFile {
                                let span = self.pp.shub.walk_up_to_base_source(token.span);
                                let LocOrigin::Source(id) = self.pp.shub.span_origin(span) else {
                                    return;
                                };
                                self.pp
                                    .shub
                                    .source(id)
                                    .path()
                                    .as_os_str()
                                    .as_encoded_bytes()
                            } else {
                                let span = self.pp.shub.walk_up_to_source(token.span);
                                let file_name = self.pp.shub.presumed_full_loc(span.hi).file_name;
                                match file_name {
                                    FileName::Path(path) => {
                                        ByteStr::new(if *builtin == BuiltinMac::FileName {
                                            path.file_name().unwrap_or_default().as_encoded_bytes()
                                        } else {
                                            path.as_os_str().as_encoded_bytes()
                                        })
                                    }
                                    FileName::Str(s) => s,
                                }
                            };
                            // todo: escape uniquement si nécessaire
                            let file_name: Vec<_> =
                                escape_stringize(file_name.iter().copied()).collect();

                            let mut text = String::with_capacity(file_name.len() + 2);
                            text.push('"');
                            text.push_str(&String::from_utf8_lossy(&file_name));
                            text.push('"');

                            let mut lexer =
                                Lexer::new(&text, self.pp.shub.write_virtual_source(&text));
                            Token {
                                space_before: token.space_before,
                                ..lexer.lex()
                            }
                        }
                        BuiltinMac::HasExpr => {
                            if self.forbid_has_exprs {
                                self.pp.diags.emit(ForbiddenHasExpr {
                                    name,
                                    span: token.span,
                                });
                            } else {
                                // on ajoute les __has_* tels quels, il sont gérés
                                // plus tard
                                out.push(token);
                            }
                            return;
                        }
                        BuiltinMac::PragmaOp => {
                            // géré plus tard
                            self.saw_pragma = true;
                            out.push(token);
                            return;
                        }
                        BuiltinMac::Counter => {
                            let counter = self.pp.counter.to_string();
                            self.pp.counter += 1;
                            let span = to_span(
                                0..counter.len() as u32,
                                self.pp.shub.write_virtual_source(&counter),
                            );
                            Token::new(TokenKind::Number, span, token.space_before)
                        }
                        BuiltinMac::IncludeLevel => {
                            // on recalcule la depth mais le InnerPreprocessor la
                            // connaît, il pourrait nous la passer aussi mais bon
                            let depth = self.pp.shub.include_depth(token.span.lo).to_string();
                            let span = to_span(
                                0..depth.len() as u32,
                                self.pp.shub.write_virtual_source(&depth),
                            );
                            Token::new(TokenKind::Number, span, token.space_before)
                        }
                        BuiltinMac::Timestamp => {
                            // todo: vrai timestamp (il faudrait que le file loader
                            // puisse nous retourner l'info)
                            let text = "\"??? ??? ?? ??:??:?? ????\"";
                            let mut lexer =
                                Lexer::new(text, self.pp.shub.write_virtual_source(text));
                            Token {
                                space_before: token.space_before,
                                ..lexer.lex()
                            }
                        }
                    };

                    let subst = self.pp.shub.add_subst(
                        SubstKind::MacExpansion {
                            name,
                            name_span: None,
                        },
                        token.span,
                        expanded.span,
                    );
                    expanded.span = expanded.span.apply_subst(subst);
                    out.push(expanded);
                }
            }
        } else {
            out.push(token);
        }
    }

    fn parse_args(
        &mut self,
        token: &Token,
        name: Name,
        name_span: Option<Span>,
        params: &[MacParam],
        variadic: bool,
        out: &mut Vec<Token>,
        mut lexer: Option<&mut Lexer>,
    ) -> Option<(Vec<MacArg>, Span)> {
        let prev_lexer = lexer.as_deref_mut().cloned();
        let at_bol = Cell::new(false);
        let mut went_past_stack = false;
        let mut get_next = || {
            self.exp_stack.pop_front().or_else(|| {
                if let Some(ref mut lexer) = lexer {
                    went_past_stack = true;
                    at_bol.set(lexer.at_bol());
                    Some(lexer.lex())
                } else {
                    None
                }
            })
        };

        let paren_l_span;
        if let Some(next) = get_next() {
            if next.kind == TokenKind::ParenL {
                paren_l_span = next.span;
            } else {
                out.push(token.clone());

                // on a mangé le next mais il est en fait pas pour nous, donc on
                // le régurgite pour le remettre à sa place
                if went_past_stack && let Some(lexer) = lexer {
                    *lexer = prev_lexer.unwrap();
                } else {
                    self.exp_stack.push_front(next);
                }
                return None;
            }
        } else {
            out.push(token.clone());
            return None;
        }

        let mut args = Vec::new();
        let mut tokens = Vec::new();
        let mut in_va_arg = params.first().is_some_and(|p| p.name == pp_kw::VaArgs);
        let mut nest = 0;
        let mut contains_names = false;
        let paren_r_span;

        loop {
            let next = match get_next() {
                Some(t) if t.kind != TokenKind::Eof => t,
                _ => {
                    self.pp.diags.emit(UnterminatedMacCall {
                        name,
                        span: token.span,
                    });
                    return None;
                }
            };

            match &next.kind {
                TokenKind::Hash if at_bol.get() => {
                    // todo: il faudrait aussi refuser les directive `module`,
                    // `export module`, `import`, `export import` etc
                    self.pp.diags.emit(DirectiveInMacArgs { span: next.span });
                }

                TokenKind::Comma if nest == 0 && !in_va_arg => {
                    args.push(MacArg {
                        tokens: tokens.clone(),
                        expanded: None,
                        contains_names,
                    });
                    tokens.clear();
                    contains_names = false;

                    if variadic && args.len() + 1 == params.len() {
                        in_va_arg = true;
                    }
                }

                TokenKind::ParenR if nest == 0 => {
                    paren_r_span = next.span;
                    args.push(MacArg {
                        tokens,
                        expanded: None,
                        contains_names,
                    });
                    break;
                }
                TokenKind::ParenR => {
                    nest -= 1;
                    tokens.push(next);
                }
                TokenKind::ParenL => {
                    nest += 1;
                    tokens.push(next);
                }
                TokenKind::Name(_) => {
                    contains_names = true;
                    tokens.push(next);
                }
                _ => tokens.push(next),
            }
        }

        if !in_va_arg && variadic {
            // on a pas ajouté d'arg variadique alors que la macro est
            // variadique donc on ajoute un arg vide pour avoir le bon nombre
            // d'args
            args.push(MacArg {
                tokens: Vec::new(),
                expanded: None,
                contains_names: false,
            });
        }

        let empty_single_arg = args.len() == 1 && args.first().is_some_and(|a| a.tokens.is_empty());
        let wrong_num_args = if empty_single_arg {
            params.len() > 1
        } else {
            args.len() != params.len()
        };

        if wrong_num_args {
            self.pp.diags.emit(WrongMacNumArgs {
                expected: params.len(),
                actual: args.len(),
                variadic,
                defined_at: name_span,
                args_span: self.pp.shub.merge(paren_l_span, paren_r_span),
                name,
            });
            None
        } else {
            Some((args, paren_r_span))
        }
    }

    fn expand_args(
        &mut self,
        params: &[MacParam],
        args: &mut [MacArg],
        mut prev_stack_len: i32,
        first_body_token_span: Span,
    ) {
        // on a déjà avancé jusqu'à la fin des arguments (parenthèse
        // fermante) mais quand on va expand les arguments token par token il
        // faut prétendre qu'on est encore en train de parcourir les args,
        // pour pouvoir pop les currently_expanding au bon moment donc on va
        // décrémenter prev_stack_len à la main pour faire croire qu'on parcourt
        // les args
        for (i, arg) in args.iter_mut().enumerate() {
            while let Some((name, n)) = self.currently_expanding.last()
                && prev_stack_len <= *n as i32
            {
                self.pp.mac_table.get_mut(name).unwrap().expanding = false;
                self.currently_expanding.pop();
            }

            if params.get(i).is_some_and(|p| p.requires_expansion) {
                // il est possible que les tokens dans un argument n'aient pas
                // la même origine, par exemple
                // ```
                // #define F(x) G(x + 3)
                // #define G(y) y
                // F(1 * 2)
                // ```
                // on commence à expand F(1 * 2) ce qui donne G(1 * 2 + 3) mais
                // on se retrouve avec des origines différentes :
                // G(1 * 2 + 3)
                //   ----- ^^^ provient du body de F
                //   |
                //   provient de la substitution de `x`
                //
                // du coup quand on va ensuite expand `y` dans le body de G, on ne
                // peut pas créer une seule substitution qui pointe vers ces tokens
                // car les spans doivent être contigus avec la même origine, donc
                // on va créer une substitution pour chaque origine
                //
                // du coup ici pour la substitution de `y` on aura deux substitutions,
                // une qui pointe vers `1 * 2` et l'autre vers `+ 3`, et les deux
                // auront comme origine commune le `y`
                let mut arg_tokens = arg.tokens.clone();
                let ranges = if arg_tokens.is_empty() {
                    Vec::new()
                } else {
                    group_by_origin(&arg_tokens, self.pp.shub)
                };

                for r in ranges {
                    let tokens = &mut arg_tokens[r];
                    debug_assert!(!tokens.is_empty());
                    let body_span = Span {
                        lo: tokens.first().unwrap().span.lo,
                        hi: tokens.last().unwrap().span.hi,
                    };

                    // on dit que la substitution se produit au niveau du premier
                    // token du body mais c'est un mensonge
                    //
                    // le problème c'est qu'on ne sait pas encore où va se faire
                    // expand le param mais on a besoin de l'expand tout de suite
                    // (et accessoirement c'est mieux de le faire qu'une seule fois
                    // au lieu de à chaque occurrence du param dans le body)
                    //
                    // si on voulait que l'origine soit correcte il faudrait lors
                    // de la substitution de l'arg "reparenter" les tokens pour
                    // qu'ils aient comme origine le span du token correspondant
                    // à l'occurence en question du param (au lieu du span du
                    // premier token comme on fait ici)
                    // donc autrement dit recréer toute la chaîne d'origines, mais
                    // ça fait du travail en plus pour pas grand chose, même pour
                    // les messages d'erreur on s'en fout de savoir exactement
                    // où l'arg a été expandé
                    //
                    // par contre il faut bien que le span du premier token du body
                    // ait déjà comme origine l'expansion de la macro en cours
                    debug_assert_matches!(
                        self.pp.shub.span_origin(first_body_token_span),
                        LocOrigin::Subst(id) if matches!(
                            self.pp.shub.subst(id).kind,
                            SubstKind::MacExpansion { .. }
                        )
                    );
                    let subst =
                        self.pp
                            .shub
                            .add_subst(SubstKind::Other, first_body_token_span, body_span);

                    for t in tokens {
                        t.span = t.span.apply_subst(subst);
                    }
                }

                arg.expanded = if !arg.contains_names {
                    // pas la peine d'expand dans ce cas
                    // todo: aucun des tests n'arrive à prouver que c'est
                    // nécessaire de décrémenter dans ce cas, techniquement c'est
                    // plus correct mais si ça sert vraiment à rien on pourrait
                    // l'enlever
                    prev_stack_len -= arg.tokens.len() as i32 + 1;
                    Some(arg_tokens)
                } else {
                    let mut expander = MacExpander::new(self.pp, self.forbid_has_exprs);
                    expander.expanding_args = true;

                    for t in arg_tokens.iter().rev() {
                        expander.exp_stack.push_front(t.clone());
                    }

                    let mut expanded_arg = Vec::new();
                    let mut next_needs_space = false;
                    let mut old_len = expander.exp_stack.len();
                    while let Some(mut next) = expander.exp_stack.pop_front() {
                        while let Some((name, n)) = self.currently_expanding.last()
                            && prev_stack_len <= *n as i32
                        {
                            expander.pp.mac_table.get_mut(name).unwrap().expanding = false;
                            self.currently_expanding.pop();
                        }

                        // todo: trouver un test qui prouve qu'on a besoin du |
                        // (ou alors on n'en a pas besoin ?)
                        next.frozen |= matches!(
                            next.kind,
                            TokenKind::Name(name) if expander.pp.mac_table.get(&name).is_some_and(|m| m.expanding)
                        );

                        next.space_before |= next_needs_space;
                        next_needs_space = false;
                        expander.expand(&mut expanded_arg, next, None, &mut next_needs_space);

                        let new_len = expander.exp_stack.len();
                        prev_stack_len -= (old_len - new_len) as i32;
                        old_len = new_len;
                    }

                    // - 1 pour la virgule après l'arg
                    prev_stack_len -= 1;

                    Some(expanded_arg)
                }
            } else {
                // + 1 pour la virgule
                prev_stack_len -= arg.tokens.len() as i32 + 1;
            }
        }
    }

    fn concat_and_rescan(
        &mut self,
        mut tokens: Vec<Token>,
        token_expanding: Token,
        mac_name: Name,
        has_concats: bool,
        out: &mut Vec<Token>,
        mut lexer: Option<&mut Lexer>,
        next_needs_space: &mut bool,
    ) {
        let space_after_concat = if has_concats {
            tokens = apply_concats(&tokens, self.pp.shub, self.pp.diags);
            // on note qu'il faudra peut-être un espace pour le token suivant,
            // après l'expansion
            tokens
                .last()
                .is_some_and(|t| t.placemarker && t.space_before)
        } else {
            false
        };

        // on remplit la stack d'expansion au préalable (l'expansion peut manger
        // autant de tokens qu'elle veut, par ex si c'est un appel de macro ça
        // va vouloir parser les args etc)
        let n = self.exp_stack.len() as u32;
        let mut it = tokens.into_iter().rev();
        while let Some(t) = it.next() {
            if t.placemarker {
                continue;
            }

            let mut space_before = t.space_before;
            if let Some(next) = it.clone().next()
                && next.placemarker
            {
                it.next();
                space_before |= next.space_before;
            }

            self.exp_stack.push_front(Token { space_before, ..t });
        }

        let prev_len = out.len();
        self.pp.mac_table.get_mut(&mac_name).unwrap().expanding = true;
        self.currently_expanding.push((mac_name, n));

        // on peut maintenant expand
        while self.exp_stack.len() as u32 > n
            && let Some(mut next) = self.exp_stack.pop_front()
        {
            next.space_before |= *next_needs_space;
            *next_needs_space = false;

            // todo: est-ce que un if ne suffirait pas ? aucun test ne prouve
            // qu'il faut un while mais who knows
            while let Some((name, n)) = self.currently_expanding.last()
                && (self.exp_stack.len() as u32) < *n
            {
                self.pp.mac_table.get_mut(name).unwrap().expanding = false;
                self.currently_expanding.pop();
            }

            next.frozen |= matches!(
                next.kind,
                TokenKind::Name(name) if self.pp.mac_table.get(&name).is_some_and(|m| m.expanding)
            );

            self.expand(out, next, lexer.as_deref_mut(), next_needs_space);
        }

        // todo: on doit aller rechercher la macro, on ne peut pas utiliser la
        // référence qu'on avait juste avant la boucle mais ça serait bien d'une
        // manière ou d'une autre qu'on ait pas besoin de le chercher 2 fois
        self.pp.mac_table.get_mut(&mac_name).unwrap().expanding = false;
        self.currently_expanding.pop();

        *next_needs_space |=
            out.len() == prev_len && token_expanding.space_before || space_after_concat;
    }
}

fn subst_args(
    tokens: &[Token],
    params: &[MacParam],
    args: &[MacArg],
    shub: &mut SourceHub,
    diags: &mut Diags,
) -> Vec<Token> {
    let get_arg = |name,
                   it: &mut Iter<'_, Token>,
                   shub: &mut SourceHub,
                   diags: &mut Diags,
                   in_concat_or_stringize: bool| {
        match name {
            _ if let Some(i) = find_param(params, name) => {
                Some(Cow::Borrowed(if in_concat_or_stringize {
                    &args[i].tokens
                } else {
                    args[i]
                        .expanded
                        .as_ref()
                        .expect("l'arg doit avoir été expandé")
                }))
            }
            pp_kw::VaOpt => {
                // pas besoin de gérer l'erreur car elle aura déjà été détectée
                // lors du define, ici on suppose que c'est bien formé, sinon
                // on retourne juste un Vec vide
                let va_opt_tokens = parse_parens(it).map(|p| p.tokens).unwrap_or(Vec::new());
                let va_arg = args.last().expect("il doit y avoir un va_arg");
                if va_arg
                    .expanded
                    .as_ref()
                    .expect("va_arg doit avoir été expandé")
                    .is_empty()
                {
                    return Some(Cow::Owned(Vec::new()));
                }

                let mut tokens = subst_args(&va_opt_tokens, params, args, shub, diags);
                let has_concats = tokens
                    .iter()
                    .find(|t| t.kind == TokenKind::HashHash)
                    .is_some();

                if has_concats {
                    tokens = apply_concats(&tokens, shub, diags)
                }

                if tokens.len() > 1 && tokens.iter().all(|t| t.placemarker) {
                    // on ne veut pas plusieurs placemarkers donc on en garde qu'un
                    tokens.drain(1..);
                }

                Some(Cow::Owned(tokens))
            }
            _ => None,
        }
    };

    let subst_arg = |token: &Token, arg_tokens: &[Token], out: &mut Vec<Token>| {
        if let [first, rest @ ..] = arg_tokens {
            out.push(Token {
                space_before: token.space_before,
                ..first.clone()
            });
            out.extend_from_slice(rest);
        } else {
            out.push(Token {
                kind: TokenKind::Unknown,
                placemarker: true,
                ..token.clone()
            });
        }
    };

    let mut out = Vec::new();
    let mut it = tokens.iter();
    while let Some(curr) = it.next() {
        let curr = curr.clone();
        match curr.kind {
            TokenKind::Hash => {
                // on mange systématiquement le next même si il se trouve que
                // c'était pas un param car ça serait de toute façon une erreur
                // (détectée lors du #define)
                if let Some(next) = it.next()
                    && let TokenKind::Name(name) = next.kind
                {
                    let Some(arg) = get_arg(name, &mut it, shub, diags, true) else {
                        continue;
                    };
                    let expanded_at = shub.merge(curr.span, next.span);

                    match stringize(&arg, curr.space_before, shub) {
                        Ok(mut token) => {
                            let subst = shub.add_subst(SubstKind::Other, expanded_at, token.span);
                            token.span = token.span.apply_subst(subst);
                            out.push(token);
                        }
                        Err(lexeme) => {
                            let arg_span = find_param(params, name).map(|i| {
                                let tokens = &args[i].tokens;
                                shub.merge(
                                    tokens.first().unwrap().span,
                                    tokens.last().unwrap().span,
                                )
                            });
                            diags.emit(InvalidStringize {
                                stringize_span: expanded_at,
                                arg_span,
                                lexeme,
                            });
                        }
                    }
                }
            }

            TokenKind::HashHash => {
                out.push(curr);

                let old_it = it.clone();
                if let Some(next) = it.next()
                    && let TokenKind::Name(name) = next.kind
                    && let Some(arg) = get_arg(name, &mut it, shub, diags, true)
                {
                    subst_arg(next, &arg, &mut out);
                } else {
                    // si c'était pas un arg il faut pas le manger donc on revient
                    // en arrière
                    it = old_it;
                }
            }

            TokenKind::Name(name) => {
                let in_concat = it
                    .clone()
                    .next()
                    .is_some_and(|next| next.kind == TokenKind::HashHash);

                let Some(arg) = get_arg(name, &mut it, shub, diags, in_concat) else {
                    out.push(curr);
                    continue;
                };

                subst_arg(&curr, &arg, &mut out);
            }

            _ => out.push(curr),
        }
    }

    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ParseParensError {
    NoParenL,
    NoParenR(Span), // span de la parenthèse ouvrante
}

struct ParsedParens {
    tokens: Vec<Token>,
    l_span: Span,
    r_span: Span,
}

// todo: cette fonction pourrait être un itérateur plutôt, pour éviter de créer
// un Vec quand on n'en a pas besoin
fn parse_parens<'a>(
    it: &mut (impl Iterator<Item = &'a Token> + Clone),
) -> Result<ParsedParens, ParseParensError> {
    let l_span = if let Some(next) = it.clone().next()
        && next.kind == TokenKind::ParenL
    {
        it.next();
        next.span
    } else {
        return Err(ParseParensError::NoParenL);
    };

    let mut tokens = Vec::new();
    let mut nest = 0;
    for next in it {
        match next.kind {
            TokenKind::ParenR if nest == 0 => {
                return Ok(ParsedParens {
                    tokens,
                    l_span,
                    r_span: next.span,
                });
            }
            TokenKind::ParenR => {
                nest -= 1;
                tokens.push(next.clone());
            }
            TokenKind::ParenL => {
                nest += 1;
                tokens.push(next.clone());
            }
            _ => tokens.push(next.clone()),
        }
    }

    Err(ParseParensError::NoParenR(l_span))
}

struct ParsedEmbedParam {
    prefix: Option<Name>,
    name: Name,
    name_span: Span,
    parens: Option<ParsedParens>,
}

fn parse_embed_params(
    tokens: &[Token],
    shub: &SourceHub,
    diags: &mut Diags,
) -> Vec<ParsedEmbedParam> {
    // d'après [cpp.pre] paragraphe 4, si les tokens contiennent l'identifier
    // limit, prefix, suffix ou if_empty (avant expansion) et qu'il existe une
    // macro du même nom, le programme est ill-formed
    // dans notre cas, les tokens ont déjà été expandés donc on se demande juste
    // si ils proviennent d'une macro nommée limit, etc, c'est à peu près équivalent
    //
    // le set permet d'éviter d'avoir plusieurs fois la même erreur (il peut y
    // avoir plusieurs tokens qui viennent de la même macro)
    let mut set = HashSet::new();
    for t in tokens {
        let span = shub.walk_up_to_source(t.span);
        let names = &[pp_kw::Limit, pp_kw::Prefix, pp_kw::Suffix, pp_kw::IfEmpty];
        if !set.contains(&span)
            && let Some((name, defined_at)) = last_mac_expansion(t.span, shub)
            && names.contains(&name)
        {
            set.insert(span);
            diags.emit(ExpandedStandardEmbedParam {
                name,
                expanded_at: span,
                defined_at,
            });
        }
    }

    let mut params = Vec::new();
    let mut it = tokens.iter();
    while let Some(curr) = it.next() {
        let TokenKind::Name(name) = curr.kind else {
            diags.emit(ExpectedEmbedParam {
                span: curr.span,
                has_name: true,
            });
            break;
        };

        let peek = it.clone().next();
        let param = if peek.is_some_and(|t| t.kind == TokenKind::ColonColon) {
            it.next();

            let Some(next) = it.next() else {
                diags.emit(ExpectedEmbedParam {
                    span: peek.unwrap().span,
                    has_name: false,
                });
                break;
            };

            let TokenKind::Name(next_name) = next.kind else {
                diags.emit(ExpectedEmbedParam {
                    span: next.span,
                    has_name: true,
                });
                break;
            };

            let parens = match parse_parens(&mut it) {
                Ok(parens) => Some(parens),
                Err(ParseParensError::NoParenL) => None,
                Err(ParseParensError::NoParenR(span)) => {
                    diags.emit(UnmatchedParenL { span });
                    break;
                }
            };

            ParsedEmbedParam {
                prefix: Some(name),
                name: next_name,
                name_span: shub.merge(curr.span, next.span),
                parens,
            }
        } else if peek.is_some_and(|t| t.kind == TokenKind::ParenL) {
            let parens = match parse_parens(&mut it) {
                Ok(parens) => Some(parens),
                Err(ParseParensError::NoParenL) => unreachable!(),
                Err(ParseParensError::NoParenR(span)) => {
                    diags.emit(UnmatchedParenL { span });
                    break;
                }
            };

            ParsedEmbedParam {
                prefix: None,
                name,
                name_span: curr.span,
                parens,
            }
        } else {
            diags.emit(ExpectedOperandInParens {
                operator: pp_kw::Embed,
                span: curr.span,
                has_parens: false,
            });
            continue;
        };

        if check_balanced(&param, diags) {
            params.push(param);
        }
    }

    params
}

fn check_balanced(param: &ParsedEmbedParam, diags: &mut Diags) -> bool {
    let Some(parens) = &param.parens else {
        return true;
    };
    let mut left = None;
    let mut brace_nest = 0;
    let mut bracket_nest = 0;

    for t in &parens.tokens {
        match t.kind {
            TokenKind::BraceL => {
                brace_nest += 1;
                left = Some(t.clone());
            }
            TokenKind::BracketL => {
                bracket_nest += 1;
                left = Some(t.clone());
            }
            TokenKind::BraceR | TokenKind::BracketR => {
                let (expected, nest) = if t.kind == TokenKind::BraceR {
                    brace_nest -= 1;
                    (TokenKind::BraceL, brace_nest)
                } else {
                    bracket_nest -= 1;
                    (TokenKind::BracketL, bracket_nest)
                };

                let orphan = match &left {
                    Some(l) if l.kind == expected => {
                        left = None;
                        continue;
                    }
                    Some(l) if nest >= 0 => l.clone(),
                    _ => t.clone(),
                };

                diags.emit(UnbalancedEmbedParam {
                    kind: orphan.kind,
                    span: orphan.span,
                    param: param.name,
                });
                return false;
            }
            _ => {}
        }
    }

    if let Some(left) = left {
        diags.emit(UnbalancedEmbedParam {
            kind: left.kind,
            span: left.span,
            param: param.name,
        });
        return false;
    }

    true
}

#[derive(Default)]
struct EmbedParams {
    limit: Option<usize>,
    prefix: Option<Vec<Token>>,
    suffix: Option<Vec<Token>>,
    if_empty: Option<Vec<Token>>,
}

fn extract_embed_params(
    parsed_params: Vec<ParsedEmbedParam>,
    forbid_unknowns: bool,
    pp: &mut Preprocessor,
) -> (EmbedParams, bool) {
    let mut params = EmbedParams::default();
    let mut saw_unknowns = false;

    let duplicate_param = |name, old, new, diags: &mut Diags| {
        diags.emit(DuplicateEmbedParam { name, old, new });
    };
    let mut limit_span = None;
    let mut prefix_span = None;
    let mut suffix_span = None;
    let mut if_empty_span = None;
    for p in parsed_params {
        match (p.prefix, p.name) {
            (None, pp_kw::Limit) => {
                if let Some(span) = limit_span {
                    duplicate_param(pp_kw::Limit, span, p.name_span, pp.diags);
                    continue;
                }
                limit_span = Some(p.name_span);

                // si il y a pas de parenthèses c'est une erreur (déjà détectée
                // lors du parse des params)
                let Some(mut parens) = p.parens else {
                    continue;
                };

                if parens.tokens.is_empty() {
                    pp.diags.emit(ExpectedOperandInParens {
                        operator: pp_kw::Embed,
                        span: pp.shub.merge(parens.l_span, parens.r_span),
                        has_parens: true,
                    });
                    continue;
                }

                for t in &mut parens.tokens {
                    match t.kind {
                        TokenKind::Name(pp_kw::Defined) => {
                            pp.diags.emit(DefinedInLimitParam { span: t.span });
                            return (params, saw_unknowns);
                        }
                        TokenKind::Name(kw::True | kw::False) => {}
                        TokenKind::Name(name) if !pp.mac_table.contains_key(&name) => {
                            t.kind = TokenKind::Name(kw::False);
                        }
                        _ => {}
                    }
                }

                let value = match ExprParser::new(&parens.tokens, pp.shub, pp.diags).parse() {
                    Ok(ops) => Interpreter::new().eval(&ops),
                    Err(errors) => {
                        pp.diags.emit(InvalidExpr(errors, pp_kw::Limit));
                        0
                    }
                };

                if value < 0 {
                    let first = parens.tokens.first().unwrap();
                    let last = parens.tokens.last().unwrap();
                    let span = pp.shub.merge(first.span, last.span);
                    pp.diags.emit(NegativeEmbedLimit { span, value });
                    continue;
                }

                params.limit = usize::try_from(value).ok();
            }
            (None, pp_kw::Prefix) => {
                if let Some(span) = prefix_span {
                    duplicate_param(pp_kw::Prefix, span, p.name_span, pp.diags);
                    continue;
                }
                prefix_span = Some(p.name_span);
                params.prefix = p.parens.map(|p| p.tokens);
            }
            (None, pp_kw::Suffix) => {
                if let Some(span) = suffix_span {
                    duplicate_param(pp_kw::Suffix, span, p.name_span, pp.diags);
                    continue;
                }
                suffix_span = Some(p.name_span);
                params.suffix = p.parens.map(|p| p.tokens);
            }
            (None, pp_kw::IfEmpty) => {
                if let Some(span) = if_empty_span {
                    duplicate_param(pp_kw::IfEmpty, span, p.name_span, pp.diags);
                    continue;
                }
                if_empty_span = Some(p.name_span);
                params.if_empty = p.parens.map(|p| p.tokens);
            }
            (prefix, name) => {
                saw_unknowns = true;
                if forbid_unknowns {
                    pp.diags.emit(UnknownEmbedParam {
                        prefix,
                        name,
                        span: p.name_span,
                    });
                }
            }
        }
    }

    (params, saw_unknowns)
}

fn last_mac_expansion(span: Span, shub: &SourceHub) -> Option<(Name, Option<Span>)> {
    match shub.span_origin(span) {
        LocOrigin::Source(_) => None,
        LocOrigin::Subst(id) => {
            if let SubstKind::MacExpansion { name, name_span } = shub.subst(id).kind {
                Some((name, name_span))
            } else {
                None
            }
        }
    }
}

struct UnescapePragma<'a> {
    chars: Chars<'a>,
}

impl Iterator for UnescapePragma<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self.chars.next() {
            Some('\\') if let Some(c @ ('"' | '\\')) = self.chars.clone().next() => {
                self.chars.next();
                Some(c)
            }
            next => next,
        }
    }
}

struct EscapeStringize<I: Iterator<Item = u8> + Clone> {
    inner: I,
    next: Option<u8>,
}

impl<I: Iterator<Item = u8> + Clone> Iterator for EscapeStringize<I> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        if let Some(next) = self.next {
            self.next = None;
            return Some(next);
        }

        // todo: on fait pareil que lex::eat_newline mais avec des u8, ça serait bien
        // de le refactoriser
        let mut eat_newline = || {
            let mut it = self.inner.clone();
            match it.next() {
                Some(b'\r') if let Some(b'\n') = it.clone().next() => {
                    it.next();
                }
                Some(b'\r' | b'\n') => {}
                _ => return false,
            }

            self.inner = it;
            true
        };

        if eat_newline() {
            self.next = Some(b'n');
            return Some(b'\\');
        }

        match self.inner.next() {
            Some(b'"') => {
                self.next = Some(b'"');
                Some(b'\\')
            }
            Some(b'\\') => {
                self.next = Some(b'\\');
                Some(b'\\')
            }
            next => next,
        }
    }
}

fn escape_stringize<I: Iterator<Item = u8> + Clone>(it: I) -> EscapeStringize<I> {
    EscapeStringize {
        inner: it,
        next: None,
    }
}

fn stringize(tokens: &[Token], space_before: bool, shub: &mut SourceHub) -> Result<Token, String> {
    // todo: temp alloc
    let mut text = String::with_capacity(tokens.len() * 3);
    let mut space = false;
    let mut it = tokens.iter();

    text.push('"');
    while let Some(curr) = it.next() {
        if curr.placemarker {
            continue;
        }

        if space {
            text.push(' ');
        }
        space = it.clone().next().is_some_and(|t| t.space_before);

        let lexeme = curr.lexeme(shub);
        match curr.kind {
            TokenKind::Char(..) | TokenKind::Str(..) | TokenKind::Header(..) => {
                text.extend(escape_stringize(lexeme.bytes()).map(|b| b as char));
            }
            _ => text.push_str(&lexeme),
        }
    }
    text.push('"');

    let mut lexer = Lexer::new(&text, shub.write_virtual_source(&text));
    let token = lexer.lex();

    if lexer.lex().kind != TokenKind::Eof || !lexer.errors().is_empty() {
        // todo: on pourrait retourner les erreurs pour les afficher
        Err(text)
    } else {
        Ok(Token {
            space_before,
            ..token
        })
    }
}

fn concat(lhs: &Token, rhs: &Token, shub: &mut SourceHub) -> Result<Token, ()> {
    match (lhs.placemarker, rhs.placemarker) {
        (true, true) | (false, true) => return Ok(lhs.clone()),
        (true, false) => {
            return Ok(Token {
                space_before: lhs.space_before,
                ..rhs.clone()
            });
        }
        _ => {}
    }

    let text = [lhs.lexeme(shub), rhs.lexeme(shub)].join("");
    let mut lexer = Lexer::new(&text, shub.write_virtual_source(&text));
    let token = lexer.lex();

    if token.kind == TokenKind::Eof
        || lexer.lex().kind != TokenKind::Eof
        || !lexer.errors().is_empty()
    {
        // todo: on pourrait retourner les erreurs
        Err(())
    } else {
        Ok(Token {
            space_before: lhs.space_before,
            ..token
        })
    }
}

fn apply_concats(tokens: &[Token], shub: &mut SourceHub, diags: &mut Diags) -> Vec<Token> {
    // todo: temp alloc
    let mut vec = Vec::with_capacity(tokens.len());
    let mut it = tokens.iter();

    while let Some(curr) = it.next() {
        let mut curr = curr.clone();

        while let Some(next) = it.clone().next()
            && next.kind == TokenKind::HashHash
        {
            it.next();
            let hash_hash_span = next.span;

            if let Some(next_next) = it.next() {
                match concat(&curr, next_next, shub) {
                    Ok(token) => {
                        // on considère qu'on a substitué là où se trouve le ##
                        // et pas toute la concaténation (par ex entre `a` et `c`
                        // dans `a ## b ## c`) parce que c'est vraiment pas utile,
                        // sinon il faudrait prendre l'origine commune entre le
                        // premier et dernier token etc c'est chiant pour rien
                        let subst = shub.add_subst(SubstKind::Other, hash_hash_span, token.span);
                        curr = Token {
                            span: token.span.apply_subst(subst),
                            ..token
                        }
                    }
                    Err(()) => {
                        diags.emit(InvalidConcat {
                            lhs_lexeme: &curr.lexeme(shub),
                            rhs_lexeme: &next_next.lexeme(shub),
                            hash_hash_span,
                        });
                    }
                }
            }
        }

        vec.push(curr);
    }

    vec
}

/// retourne les ranges dans le slice de tokens telles que tous les tokens d'une
/// range ont la même origine
// todo: temp alloc ou retourner un itérateur ?
fn group_by_origin(tokens: &[Token], shub: &SourceHub) -> Vec<Range<usize>> {
    debug_assert!(!tokens.is_empty());

    let mut ranges = Vec::new();
    let mut start = 0;
    let mut origin = shub.span_origin(tokens.first().unwrap().span);

    for (i, t) in tokens.iter().skip(1).enumerate() {
        let next_origin = shub.span_origin(t.span);
        if next_origin != origin {
            ranges.push(start..i + 1);
            start = i + 1;
            origin = next_origin;
        }
    }

    ranges.push(start..tokens.len());

    // on vérifie que toutes les ranges cumulées couvrent bien tous les tokens,
    // sans overlap
    #[cfg(debug_assertions)]
    {
        let mut covered = 0;
        let mut prev_end = 0;

        for r in &ranges {
            covered += r.len();
            debug_assert_eq!(r.start, prev_end);
            prev_end = r.end;
        }
        debug_assert_eq!(covered, tokens.len());
    }

    ranges
}

pub struct ExprParser<'a> {
    tokens: &'a [Token],
    shub: &'a SourceHub,
    diags: &'a mut Diags,
    ops: Vec<Op>,
    errors: Vec<ExprError>,
    prev: Token,
    eof: bool,
}

impl<'a> ExprParser<'a> {
    pub fn new(tokens: &'a [Token], shub: &'a SourceHub, diags: &'a mut Diags) -> Self {
        Self {
            tokens,
            shub,
            diags,
            ops: Vec::new(),
            errors: Vec::new(),
            prev: Token::new(
                TokenKind::Eof,
                // on met un truc bidon pour le premier mais on va jamais le lire
                // donc c'est bon
                Span {
                    lo: Loc(0),
                    hi: Loc(0),
                },
                false,
            ),
            eof: false,
        }
    }

    pub fn parse(mut self) -> Result<Vec<Op>, Vec<ExprError>> {
        debug_assert!(!self.tokens.is_empty());
        self.expr(0);

        // si il y a encore des tokens c'est pas bon, sinon on aurait mangé toute
        // l'expression
        if self.peek().kind != TokenKind::Eof {
            let span = self.peek().span;
            self.errors.push(if self.peek().kind == TokenKind::ParenR {
                ExprError::UnmatchedParen {
                    span,
                    is_left: false,
                }
            } else {
                ExprError::UnexpectedToken(self.peek().span)
            });
        }

        if self.errors.is_empty() {
            Ok(self.ops)
        } else {
            Err(self.errors)
        }
    }

    fn peek(&self) -> Token {
        if let Some((next, _)) = self.tokens.split_first() {
            next.clone()
        } else {
            Token {
                kind: TokenKind::Eof,
                ..self.prev.clone()
            }
        }
    }

    fn bump(&mut self) -> Token {
        let next = self.peek();
        self.tokens.split_off_first();
        self.prev = next;
        self.prev.clone()
    }

    fn expr(&mut self, min_prec: i32) {
        self.primary();

        while precedence(&self.peek().kind) > min_prec {
            let kind = self.peek().kind.clone();
            self.bump();
            match kind {
                TokenKind::Question => self.cond(),
                _ => self.binary(),
            }
        }
    }

    fn primary(&mut self) {
        use TokenKind::*;

        match self.bump().kind {
            Name(kw::True) => self.ops.push(Op::Value(1)),
            Name(kw::False) => self.ops.push(Op::Value(0)),
            Number => self.number(),
            Str(..) => self.errors.push(ExprError::Str(self.prev.span)),
            ParenL => self.paren(),
            Char(_, value, ud_suffix) | Multichar(value, ud_suffix) => {
                if ud_suffix.is_some() {
                    self.errors.push(ExprError::UdSuffix(self.prev.span));
                }
                self.ops.push(Op::Value(value as i128));
            }
            Eof => {
                if !self.eof {
                    self.eof = true;
                    self.errors.push(ExprError::ExpectedExpr(self.prev.span));
                }
            }
            _ => self.unary(),
        }
    }

    fn number(&mut self) {
        debug_assert_eq!(self.prev.kind, TokenKind::Number);

        let lexeme = &self.prev.lexeme(self.shub);
        match parse_number(lexeme) {
            Ok(number) => match number.kind {
                NumberLitKind::Float { .. } => self.errors.push(ExprError::Float(self.prev.span)),
                NumberLitKind::Int { value, .. } => {
                    if number.ud_suffix.is_some() {
                        self.errors.push(ExprError::UdSuffix(self.prev.span));
                    }
                    self.ops.push(Op::Value(value));
                }
            },
            Err(e) => self.diags.emit(e.into_diag(lexeme, self.prev.span)),
        }
    }

    fn paren(&mut self) {
        debug_assert_eq!(self.prev.kind, TokenKind::ParenL);
        let paren_l_span = self.prev.span;

        if self.peek().kind == TokenKind::ParenR {
            self.bump();
            let span = self.shub.merge(paren_l_span, self.peek().span);
            self.errors.push(ExprError::EmptyParens(span));
            return;
        }

        self.expr(0);

        if self.bump().kind != TokenKind::ParenR {
            self.errors.push(ExprError::UnmatchedParen {
                span: paren_l_span,
                is_left: true,
            });
        }
    }

    fn unary(&mut self) {
        let op = self.prev.clone();
        if op.kind == TokenKind::ParenR {
            self.errors.push(ExprError::UnmatchedParen {
                span: op.span,
                is_left: false,
            });
            return;
        }
        self.primary();

        match op.kind {
            TokenKind::Plus => {}
            TokenKind::Minus => self.ops.push(Op::Neg),
            TokenKind::Bang => self.ops.push(Op::Not),
            TokenKind::Tilde => self.ops.push(Op::BitNot),

            _ => {
                let kind = match op.kind {
                    TokenKind::And => UnOpKind::AddrOf,
                    TokenKind::Star => UnOpKind::Deref,
                    _ => UnOpKind::Other,
                };
                self.errors.push(ExprError::InvalidUnOp(op.span, kind));
            }
        }
    }

    fn binary(&mut self) {
        use TokenKind::*;

        let op = self.prev.clone();
        self.expr(precedence(&op.kind));

        match op.kind {
            EqEq => self.ops.push(Op::Eq),
            Ne => self.ops.push(Op::Ne),
            Lt => self.ops.push(Op::Lt),
            Gt => self.ops.push(Op::Gt),
            LtEq => self.ops.push(Op::Le),
            GtEq => self.ops.push(Op::Ge),

            Plus => self.ops.push(Op::Add),
            Minus => self.ops.push(Op::Sub),
            Star => self.ops.push(Op::Mul),
            Slash => self.ops.push(Op::Div),
            Percent => self.ops.push(Op::Rem),
            Caret => self.ops.push(Op::Xor),
            LtLt => self.ops.push(Op::Shl),
            GtGt => self.ops.push(Op::Shr),
            AndAnd => self.ops.push(Op::And),
            OrOr => self.ops.push(Op::Or),
            And => self.ops.push(Op::BitAnd),
            Or => self.ops.push(Op::BitOr),

            _ => {
                let kind = match op.kind {
                    ParenL => {
                        // on mange l'éventuelle parenthèse fermante pour pas avoir
                        // une erreur de plus par rapport à ça
                        if self.peek().kind == TokenKind::ParenR {
                            self.bump();
                        }
                        BinOpKind::FnCall
                    }
                    BracketL => {
                        if self.peek().kind == TokenKind::BracketR {
                            self.bump();
                        }
                        BinOpKind::Subscript
                    }
                    Comma => BinOpKind::Comma,
                    _ if is_assign(&op.kind) => BinOpKind::Assign,
                    _ => BinOpKind::Other,
                };
                self.errors.push(ExprError::InvalidBinOp(op.span, kind));
            }
        }
    }

    fn cond(&mut self) {
        debug_assert_eq!(self.prev.kind, TokenKind::Question);
        let question_span = self.prev.span;

        if self.peek().kind == TokenKind::Colon {
            self.errors.push(ExprError::ExpectedExpr(question_span));
        } else {
            self.expr(0);
        }

        if self.bump().kind != TokenKind::Colon {
            self.errors
                .push(ExprError::QuestionWithoutColon(question_span));
            return;
        }

        self.expr(precedence(&TokenKind::Question));
        self.ops.push(Op::Cond);
    }
}

fn is_assign(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::LtLtEq
            | TokenKind::GtGtEq
            | TokenKind::AndEq
            | TokenKind::CaretEq
            | TokenKind::OrEq
    )
}

fn precedence(kind: &TokenKind) -> i32 {
    use TokenKind::*;
    match kind {
        Comma => 1,
        _ if is_assign(kind) => 2,
        Question => 2,
        OrOr => 3,
        AndAnd => 4,
        Or => 5,
        Caret => 6,
        And => 7,
        EqEq | Ne => 8,
        Lt | Gt | LtEq | GtEq => 9,
        Spaceship => 10,
        LtLt | GtGt => 11,
        Plus | Minus => 12,
        Star | Slash | Percent => 13,
        DotStar | ArrowStar => 14,
        Dot | Arrow => 15,
        BracketL | ParenL => 16,
        _ => -1,
    }
}

pub enum Op {
    Value(i128),

    Eq, // a == b
    Ne, // a != b
    Lt, // a < b
    Gt, // a > b
    Le, // a <= b
    Ge, // a >= b

    Add,    // a + b
    Sub,    // a - b
    Mul,    // a * b
    Div,    // a / b
    Rem,    // a % b
    Xor,    // a ^ b
    Shl,    // a << b
    Shr,    // a >> b
    And,    // a && b
    Or,     // a || b
    BitAnd, // a & b
    BitOr,  // a | b

    Neg,    // -a
    Not,    // !a
    BitNot, // ~a

    Cond, // a ? b : c
}

#[derive(Default)]
pub struct Interpreter {
    stack: Vec<i128>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    pub fn eval(&mut self, ops: &[Op]) -> i128 {
        let stack = &mut self.stack;

        let un = |stack: &mut Vec<i128>, f: fn(i128) -> i128| {
            let a = stack.pop().unwrap();
            stack.push(f(a));
        };

        let bin = |stack: &mut Vec<i128>, f: fn(i128, i128) -> i128| {
            let b = stack.pop().unwrap();
            let a = stack.pop().unwrap();
            stack.push(f(a, b));
        };

        for op in ops {
            use Op::*;

            match op {
                Value(v) => stack.push(*v),

                Eq => bin(stack, |a, b| (a == b) as i128),
                Ne => bin(stack, |a, b| (a != b) as i128),
                Lt => bin(stack, |a, b| (a < b) as i128),
                Gt => bin(stack, |a, b| (a > b) as i128),
                Le => bin(stack, |a, b| (a <= b) as i128),
                Ge => bin(stack, |a, b| (a >= b) as i128),

                Add => bin(stack, |a, b| a + b),
                Sub => bin(stack, |a, b| a - b),
                Mul => bin(stack, |a, b| a * b),
                Div => bin(stack, |a, b| a / b),
                Rem => bin(stack, |a, b| a % b),
                Xor => bin(stack, |a, b| a ^ b),
                Shl => bin(stack, |a, b| a << b),
                Shr => bin(stack, |a, b| a >> b),
                And => bin(stack, |a, b| (a != 0 && b != 0) as i128),
                Or => bin(stack, |a, b| (a != 0 || b != 0) as i128),
                BitAnd => bin(stack, |a, b| a & b),
                BitOr => bin(stack, |a, b| a | b),

                Neg => un(stack, |a| -a),
                Not => un(stack, |a| (a == 0) as i128),
                BitNot => un(stack, |a| !a),

                Cond => {
                    let c = stack.pop().unwrap();
                    let b = stack.pop().unwrap();
                    let a = stack.pop().unwrap();
                    stack.push(if a != 0 { b } else { c });
                }
            }
        }

        stack.first().copied().unwrap_or(0)
    }
}

impl From<LexError> for Diag {
    fn from(v: LexError) -> Self {
        use LexError::*;

        #[rustfmt::skip]
        let (title, span) = match v {
            Char(_, span) => ("Invalid character literal".to_owned(), span),
            Str(_, span) => ("Invalid string literal".to_owned(), span),
            Unterminated(ref kind, span) => {
                let kind = match kind {
                    UnterminatedKind::MultilineComment => "comment",
                    UnterminatedKind::Char => "character literal",
                    UnterminatedKind::Str => "string literal",
                    UnterminatedKind::RawStr { .. } => "raw string literal",
                };
                (format!("Unterminated {kind}"), span)
            }
            Escape(_, span) => ("Invalid escape sequence".to_owned(), span),
            UnexpectedBasicUcn { c, is_control, span } => (
                if is_control {
                    "Invalid use of universal character name".to_owned()
                } else {
                    format!("Invalid use of universal character name `{c}`")
                },
                span,
            ),
            ForbiddenVaArgs(name, span) | ForbiddenHasExpr(name, span) => (
                format!("Illegal use of `{}`", name),
                span,
            ),
        };

        #[rustfmt::skip]
        let text = match v {
            Char(error, _) => match error {
                CharError::Empty => "cannot be empty".to_owned(),
                CharError::Unmappable(encoding) => {
                    let encoding = match encoding {
                        Encoding::Ordinary | Encoding::Utf8 => "UTF-8",
                        Encoding::Wide | Encoding::Utf16 => "UTF-16",
                        Encoding::Utf32 => "UTF-32",
                    };
                    format!("character is not encodable as a single {encoding} code unit")
                }
                CharError::TooManyChars => format!("multicharacter literals cannot contain more than {MAX_MULTICHAR_LEN} characters"),
                CharError::MulticharPrefix => "multicharacter literals cannot have an encoding prefix".to_owned(),
                CharError::NonAsciiInMultichar => "multicharacter literals cannot contain non-ASCII characters".to_owned(),
            },
            Str(error, _) => match error {
                StrError::InvalidCharInDelim => "this character cannot appear in the delimiter".to_owned(),
                StrError::TooManyCharsInDelim => format!("delimiter cannot contain more than {MAX_RAW_STR_DELIM_LEN} characters"),
            },
            Unterminated(kind, _) => match kind {
                UnterminatedKind::MultilineComment => "end of comment `*/` not found".to_owned(),
                UnterminatedKind::Char => "does not end with a `'`".to_owned(),
                UnterminatedKind::Str => "does not end with a `\"`".to_owned(),
                UnterminatedKind::RawStr { delim } => {
                    if let Some(delim) = delim {
                        format!("ending sequence `){delim}\"` not found")
                    } else {
                        "ending sequence `)\"` not found".to_owned()
                    }
                }
            },
            Escape(error, _) => match error {
                EscapeError::UnknownEscape => "this escape sequence does not exist".to_owned(),
                EscapeError::OutOfRange => "this escape sequence is out of range".to_owned(),
                EscapeError::ExpectedHexDigits(n) => format!("must contain {n} hexadecimal digits"),
                EscapeError::ExpectedOpenBrace => "must specify a value inside braces `{}`".to_owned(),
                EscapeError::ExpectedOpenBraceOrHexDigit => "must specify an hexadecimal value, optionally inside braces `{}`".to_owned(),
                EscapeError::NoCloseBrace => "must end with a closing `}`".to_owned(),
                EscapeError::InvalidDigitInBraces { base } => {
                    let base = match base {
                        8 => "octal",
                        16 => "hexadecimal",
                        _ => panic!("base devrait être 8 ou 16 (au lieu de {base})"),
                    };
                    format!("must only contain {base} digits")
                }
                EscapeError::EmptyBraces => "expected a value inside the braces".to_owned(),
                EscapeError::InvalidUcnValue => "must designate a valid Unicode scalar value".to_owned(),
                EscapeError::InvalidUcnName => "must designate a valid Unicode character name".to_owned(),
            },
            UnexpectedBasicUcn { .. } => "cannot appear outside a character or string literal".to_owned(),
            ForbiddenVaArgs(..) => "cannot appear outside a variadic macro".to_owned(),
            ForbiddenHasExpr(..) => "cannot appear outside conditional directives".to_owned(),
        };

        Diag {
            kind: DiagKind::Error,
            title,
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span,
                text,
            }])],
        }
    }
}

impl ParseNumberError {
    pub fn into_diag(self, number_lexeme: &str, number_span: Span) -> Diag {
        // (on cherche le boundary car il faut pas que le span pointe en plein
        // milieu d'un caractère)
        let next_char_boundary = |mut i| loop {
            i += 1;
            if number_lexeme.is_char_boundary(i) {
                return i;
            }
        };
        let to_span = |pos| Span {
            lo: Loc(number_span.lo.0 + pos as u32),
            hi: Loc(number_span.lo.0 + next_char_boundary(pos) as u32),
        };
        let base_max = |base| match base {
            2 => ("binary", '1'),
            8 => ("octal", '7'),
            16 => ("hexadecimal", 'F'),
            _ => panic!("unknown base"),
        };

        #[rustfmt::skip]
        let (text, span) = match self {
            ParseNumberError::InvalidDigit { pos, base } => {
                let (base, max) = base_max(base);
                (format!("this is not a valid {base} digit (must be between 0 and {max})"), to_span(pos))
            }
            ParseNumberError::UnexpectedChar(pos) => ("unexpected character".to_owned(), to_span(pos)),
            ParseNumberError::InvalidSuffixStart(pos) => ("this character cannot appear at the start of the suffix".to_owned(), to_span(pos)),
            ParseNumberError::InvalidCharInSuffix(pos) => ("this character cannot appear in the suffix".to_owned(), to_span(pos)),
            ParseNumberError::EmptyNumber { base } => {
                let (base, _) = base_max(base);
                (format!("this {base} number is empty"), number_span)
            }
            ParseNumberError::ExpectedDigitBeforeQuote(pos) => ("expected a digit before this `'`".to_owned(), to_span(pos)),
            ParseNumberError::ExpectedDigitAfterQuote(pos) => ("expected a digit after this `'`".to_owned(), to_span(pos)),
            ParseNumberError::IntValueTooLarge => ("integer value too large".to_owned(), number_span),
            ParseNumberError::ExpectedExponentValue(pos) => ("expected exponent value instead of this".to_owned(), to_span(pos)),
            ParseNumberError::NoExponentInHexFloat => ("hexadecimal floating-point literals must have an exponent".to_owned(), number_span),
            ParseNumberError::EmptyHexMantissa => ("the mantissa of a hexadecimal floating-point literal cannot be empty".to_owned(), number_span),
            ParseNumberError::DotInExponent => ("the exponent must be an integer".to_owned(), number_span),
            ParseNumberError::TooManyDots => ("cannot contain more than one decimal point".to_owned(), number_span),
            ParseNumberError::BinaryFloat => ("a binary number can only be an integer".to_owned(), number_span),
            ParseNumberError::Other => ("".to_owned(), number_span),
        };

        Diag {
            kind: DiagKind::Error,
            title: "Invalid number literal".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span,
                text,
            }])],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UnOpKind {
    AddrOf,
    Deref,
    Other,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BinOpKind {
    Assign,
    FnCall,
    Subscript,
    Comma,
    Other,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ExprError {
    Float(Span),
    Str(Span),
    UdSuffix(Span),
    UnmatchedParen { span: Span, is_left: bool },
    EmptyParens(Span),
    InvalidUnOp(Span, UnOpKind),
    InvalidBinOp(Span, BinOpKind),
    UnexpectedToken(Span),
    ExpectedExpr(Span),
    QuestionWithoutColon(Span),
}

#[derive(Clone, PartialEq, Debug)]
pub struct InvalidExpr(pub Vec<ExprError>, pub Name);

impl From<InvalidExpr> for Diag {
    fn from(v: InvalidExpr) -> Self {
        let squiggles = v.0.into_iter().map(|e| {
            use ExprError::*;
            let (span, text) = match e {
                Float(span) => (span, "floating-point literals are not allowed"),
                Str(span) => (span, "string literals are not allowed"),
                UdSuffix(span) => (span, "user-defined literals are not allowed"),
                UnmatchedParen { span, is_left } => {
                    let text = if is_left {
                        "this `(` has no matching `)`"
                    } else {
                        "this `)` has no matching `(`"
                    };
                    (span, text)
                }
                EmptyParens(span) => (span, "expected expression inside the parentheses"),
                InvalidUnOp(span, kind) => {
                    let text = match kind {
                        UnOpKind::AddrOf => "address-of operator is not allowed",
                        UnOpKind::Deref => "dereference operator is not allowed",
                        UnOpKind::Other => "not a valid unary operator",
                    };
                    (span, text)
                }
                InvalidBinOp(span, kind) => {
                    let text = match kind {
                        BinOpKind::Assign => "assignment operators are not allowed",
                        BinOpKind::FnCall => "function calls are not allowed",
                        BinOpKind::Subscript => "subscript operator is not allowed",
                        BinOpKind::Comma => "comma operator is not allowed",
                        BinOpKind::Other => "not a valid binary operator",
                    };
                    (span, text)
                }
                UnexpectedToken(span) => (span, "unexpected token"),
                ExpectedExpr(span) => (span, "expected expression after this"),
                QuestionWithoutColon(span) => (span, "this `?` has no matching `:`"),
            };
            Squiggle {
                primary: true,
                span,
                text: text.to_owned(),
            }
        });
        let kind = match v.1 {
            kw::If => "#if",
            pp_kw::Elif => "#elif",
            pp_kw::Limit => "limit",
            _ => panic!("unknown name"),
        };

        Diag {
            kind: DiagKind::Error,
            title: format!("Invalid `{kind}` expression"),
            parts: vec![DiagPart::Snippet(squiggles.collect())],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct ExpectedTokensInDirective {
    pub directive: Name,
    pub span: Span,
}

impl From<ExpectedTokensInDirective> for Diag {
    fn from(v: ExpectedTokensInDirective) -> Self {
        #[rustfmt::skip]
        let thing = match v.directive {
            pp_kw::Include | pp_kw::Embed => "header name",
            kw::If | pp_kw::Elif => "expression",
            pp_kw::Define | pp_kw::Undef | pp_kw::Ifdef | pp_kw::Ifndef | pp_kw::Elifdef | pp_kw::Elifndef => "macro name",
            pp_kw::Line => "line number",
            _ => panic!("unknown directive"),
        };
        Diag {
            kind: DiagKind::Error,
            title: format!("Expected {thing}"),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: format!("after this `#{}` directive", v.directive),
            }])],
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct TokensAfterDirective {
    pub spans: Vec<Span>,
}

impl From<TokensAfterDirective> for Diag {
    fn from(v: TokensAfterDirective) -> Self {
        let (s, this) = if v.spans.len() == 1 {
            ("", "this")
        } else {
            ("s", "these")
        };

        let mut squiggles: Vec<_> = v
            .spans
            .into_iter()
            .map(|span| Squiggle {
                primary: true,
                span,
                text: "".to_owned(),
            })
            .collect();

        if let Some(last) = squiggles.last_mut() {
            last.text = format!("remove {this}");
        }

        Diag {
            kind: DiagKind::Error,
            title: format!("Extra token{s} after directive"),
            parts: vec![DiagPart::Snippet(squiggles)],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct UnmatchedParenL {
    pub span: Span,
}

impl From<UnmatchedParenL> for Diag {
    fn from(v: UnmatchedParenL) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Expected closing `)`".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "this `(` has no matching `)`".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct ExpectedOperandInParens {
    pub operator: Name,
    /// le span correspondant aux parenthèses si has_parens est true et sinon le
    /// span de l'opèrateur juste avant
    pub span: Span,
    pub has_parens: bool,
}

impl From<ExpectedOperandInParens> for Diag {
    #[rustfmt::skip]
    fn from(v: ExpectedOperandInParens) -> Self {
        let thing = match v.operator {
            pp_kw::HasInclude | pp_kw::HasEmbed => "a header name",
            pp_kw::HasCppAttribute => "an attribute",
            pp_kw::VaOpt | pp_kw::Embed => "an argument",
            pp_kw::Defined => "a macro name",
            pp_kw::PragmaOp => "a string literal",
            _ => panic!("unknown operator"),
        };
        let (title, text) = if v.has_parens {
            (format!("Expected {thing}"), "inside the parentheses")
        } else {
            (format!("Expected {thing} in parentheses `()`"), "after this")
        };

        Diag {
            kind: DiagKind::Error,
            title,
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: text.to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum HeaderError {
    Empty,
    Malformed,
    NotFound(String),
    Unreadable(String),
}

#[derive(PartialEq, Debug)]
pub struct InvalidHeader {
    pub error: HeaderError,
    pub span: Span,
}

impl From<InvalidHeader> for Diag {
    #[rustfmt::skip]
    fn from(v: InvalidHeader) -> Self {
        let title = match &v.error {
            HeaderError::Empty | HeaderError::Malformed => "Invalid header name".to_owned(),
            HeaderError::NotFound(name) | HeaderError::Unreadable(name) => format!("Unable to process header `{name}`"),
        };
        let text = match v.error {
            HeaderError::Empty => "cannot be empty",
            HeaderError::Malformed => "must have the form <filename> or \"filename\" (possibly after macro expansion)",
            HeaderError::NotFound(_) => "this file does not exist",
            HeaderError::Unreadable(_) => "this file is not readable",
        };

        Diag {
            kind: DiagKind::Error,
            title,
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: text.to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct ExceededMaxIncludeDepth {
    pub max: u32,
    pub span: Span,
}

impl From<ExceededMaxIncludeDepth> for Diag {
    fn from(v: ExceededMaxIncludeDepth) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Too many nested includes".to_owned(),
            parts: vec![
                DiagPart::Snippet(vec![Squiggle {
                    primary: true,
                    span: v.span,
                    text: "".to_owned(),
                }]),
                DiagPart::Text(format!(
                    "You can increase the limit with the `--max-include-depth` option, currently set to {}.",
                    v.max
                )),
            ],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidAttr {
    pub span: Span,
}

impl From<InvalidAttr> for Diag {
    #[rustfmt::skip]
    fn from(v: InvalidAttr) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid attribute".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "expected an identifier, optionally preceded by a namespace (e.g. `foo::bar`)".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidPragmaOperand {
    pub span: Span,
}

impl From<InvalidPragmaOperand> for Diag {
    fn from(v: InvalidPragmaOperand) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid `_Pragma` operand".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "expected a string literal".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidDirective {
    pub is_name: bool,
    pub span: Span,
}

impl From<InvalidDirective> for Diag {
    fn from(v: InvalidDirective) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid preprocessing directive".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: if v.is_name {
                    "unknown directive".to_owned()
                } else {
                    "must be an identifier".to_owned()
                },
            }])],
        }
    }
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum InvalidLineNumberKind {
    NotANumber,
    InvalidDigits,
    OutOfRange,
}

#[derive(PartialEq, Debug)]
pub struct InvalidLineNumber {
    pub kind: InvalidLineNumberKind,
    pub span: Span,
}

impl From<InvalidLineNumber> for Diag {
    fn from(v: InvalidLineNumber) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid number in `#line` directive".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: match v.kind {
                    InvalidLineNumberKind::NotANumber => "must be an integer literal",
                    InvalidLineNumberKind::InvalidDigits => "can only contain digits or `'`",
                    InvalidLineNumberKind::OutOfRange => "must be between 1 and 2'147'483'647",
                }
                .to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidLineFileName {
    pub span: Span,
}

impl From<InvalidLineFileName> for Diag {
    fn from(v: InvalidLineFileName) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid filename in `#line` directive".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "expected a string literal".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct OctalNumberInLineDirective {
    pub span: Span,
    pub value: i128,
}

impl From<OctalNumberInLineDirective> for Diag {
    fn from(v: OctalNumberInLineDirective) -> Self {
        Diag {
            kind: DiagKind::Warn,
            title: "Octal number in `#line` directive".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: format!("interpreted as `{}`", v.value),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct NoIf {
    pub directive: Name,
    pub span: Span,
}

impl From<NoIf> for Diag {
    fn from(v: NoIf) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("`#{}` without `#if`", v.directive),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "can only appear after a `#if`".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct NoEndif {
    pub directive: Name,
    pub span: Span,
}

impl From<NoEndif> for Diag {
    fn from(v: NoEndif) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("Unterminated `#{}`", v.directive),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "it has no matching `#endif`".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidDirectiveAfterElse {
    pub directive: Name,
    pub span: Span,
}

impl From<InvalidDirectiveAfterElse> for Diag {
    fn from(v: InvalidDirectiveAfterElse) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("`#{}` after `#else`", v.directive),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "cannot appear after a `#else`".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct ExpandedStandardEmbedParam {
    pub name: Name,
    pub expanded_at: Span,
    pub defined_at: Option<Span>,
}

impl From<ExpandedStandardEmbedParam> for Diag {
    fn from(v: ExpandedStandardEmbedParam) -> Self {
        let mut parts = vec![DiagPart::Snippet(vec![Squiggle {
            primary: true,
            span: v.expanded_at,
            text: "cannot appear here because it is currently defined as a macro".to_owned(),
        }])];

        if let Some(span) = v.defined_at {
            parts.push(DiagPart::Snippet(vec![Squiggle {
                primary: false,
                span,
                text: "macro defined here".to_owned(),
            }]));
        }

        // todo: ça serait bien de savoir précisément si c'est un #embed ou __has_embed
        Diag {
            kind: DiagKind::Error,
            title: format!("Illegal use of `{}` in embed", v.name),
            parts,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct ExpectedEmbedParam {
    pub span: Span,
    pub has_name: bool,
}

impl From<ExpectedEmbedParam> for Diag {
    fn from(v: ExpectedEmbedParam) -> Self {
        let text = if v.has_name {
            "expected a parameter name"
        } else {
            "expected a parameter name after this"
        };
        Diag {
            kind: DiagKind::Error,
            title: "Invalid embed parameter".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: text.to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct DuplicateEmbedParam {
    pub name: Name,
    pub old: Span,
    pub new: Span,
}

impl From<DuplicateEmbedParam> for Diag {
    fn from(v: DuplicateEmbedParam) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("Duplicate embed parameter `{}`", v.name),
            parts: vec![DiagPart::Snippet(vec![
                Squiggle {
                    primary: true,
                    span: v.new,
                    text: "cannot be specified multiple times".to_owned(),
                },
                Squiggle {
                    primary: false,
                    span: v.old,
                    text: "already specified here".to_owned(),
                },
            ])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct UnknownEmbedParam {
    pub prefix: Option<Name>,
    pub name: Name,
    pub span: Span,
}

impl From<UnknownEmbedParam> for Diag {
    fn from(v: UnknownEmbedParam) -> Self {
        let prefix = if let Some(prefix) = v.prefix {
            format!("{}::", prefix)
        } else {
            "".to_owned()
        };
        Diag {
            kind: DiagKind::Error,
            title: format!("Unknown embed parameter `{prefix}{}`", v.name),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub enum UnbalancedKind {
    Brace,
    Bracket,
}

#[derive(PartialEq, Debug)]
pub struct UnbalancedEmbedParam {
    pub kind: TokenKind,
    pub span: Span,
    pub param: Name,
}

impl From<UnbalancedEmbedParam> for Diag {
    fn from(v: UnbalancedEmbedParam) -> Self {
        let (desc, left, right) = match v.kind {
            TokenKind::BraceL => ("braces `{}`", '{', '}'),
            TokenKind::BraceR => ("braces `{}`", '}', '{'),
            TokenKind::BracketL => ("brackets `[]`", '[', ']'),
            TokenKind::BracketR => ("brackets `[]`", ']', '['),
            _ => panic!("unknown token"),
        };
        let param = v.param.as_str();

        Diag {
            kind: DiagKind::Error,
            title: format!("Unbalanced {desc} in embed parameter `{param}`"),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: format!("this `{left}` has no matching `{right}`"),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct NegativeEmbedLimit {
    pub span: Span,
    pub value: i128,
}

impl From<NegativeEmbedLimit> for Diag {
    fn from(v: NegativeEmbedLimit) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("Invalid embed limit `{}`", v.value),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "must be positive".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct DefinedAppearedAfterExpansion {
    pub span: Span,
}

impl From<DefinedAppearedAfterExpansion> for Diag {
    fn from(v: DefinedAppearedAfterExpansion) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Illegal use of `defined`".to_owned(),
            #[rustfmt::skip]
            parts: vec![
                DiagPart::Snippet(vec![Squiggle {
                    primary: true,
                    span: v.span,
                    text: "appeared after expansion of this".to_owned(),
                }]),
                DiagPart::Text("Inside a conditional directive, `defined` can only appear as an identifier.".to_owned()),
            ],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct DefinedInLimitParam {
    pub span: Span,
}

impl From<DefinedInLimitParam> for Diag {
    fn from(v: DefinedInLimitParam) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Illegal use of `defined`".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "cannot appear inside the `limit` embed parameter".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidDefinedOperand {
    /// le span correspondant à l'opérande si has_operand est true et sinon le
    /// span de l'opérateur `defined`
    pub span: Span,
    pub has_operand: bool,
    pub has_parens: bool,
}

impl From<InvalidDefinedOperand> for Diag {
    fn from(v: InvalidDefinedOperand) -> Self {
        let (title, text) = if v.has_operand {
            (
                "Invalid `defined` operand",
                if v.has_parens {
                    "expected a macro name"
                } else {
                    "expected a macro name, optionally inside parentheses `()`"
                },
            )
        } else {
            (
                "Expected macro name, optionally inside parentheses `()`",
                "after this",
            )
        };

        Diag {
            kind: DiagKind::Error,
            title: title.to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: text.to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct MacRedefined {
    pub name: Name,
    // il n'y a pas de span si la macro n'est pas définie dans un fichier
    // (définie programmatiquement)
    pub old: Option<Span>,
    pub new: Span,
}

impl From<MacRedefined> for Diag {
    fn from(v: MacRedefined) -> Self {
        let mut parts = vec![DiagPart::Snippet(vec![Squiggle {
            primary: true,
            span: v.new,
            text: "".to_owned(),
        }])];

        if let Some(span) = v.old {
            parts.push(DiagPart::Snippet(vec![Squiggle {
                primary: false,
                span,
                text: "previously defined here".to_owned(),
            }]));
        }

        parts.push(DiagPart::Text(
            "Macros can only be redefined if their bodies are identical.".to_owned(),
        ));

        Diag {
            kind: DiagKind::Error,
            title: format!("Redefinition of macro `{}`", v.name),
            parts,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct ErrorWarningDirective {
    pub is_warn: bool,
    pub span: Span,
    pub message: String,
}

impl From<ErrorWarningDirective> for Diag {
    fn from(v: ErrorWarningDirective) -> Self {
        let (kind, dir) = if v.is_warn {
            (DiagKind::Warn, "Warning")
        } else {
            (DiagKind::Error, "Error")
        };

        Diag {
            kind,
            title: if v.message.is_empty() {
                "{dir} directive".to_owned()
            } else {
                format!("{dir} directive: {}", v.message)
            },
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "here".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct RedefinedPredefMac {
    pub name: Name,
    pub span: Span,
    pub is_define: bool,
}

impl From<RedefinedPredefMac> for Diag {
    fn from(v: RedefinedPredefMac) -> Self {
        let define = if v.is_define { "redefine" } else { "undefine" };
        Diag {
            kind: DiagKind::Error,
            title: format!("Redefinition of macro `{}`", v.name),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: format!("cannot {define} builtin macro `{}`", v.name),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidMacName {
    pub lexeme: String,
    pub span: Span,
    pub is_name: bool,
}

impl From<InvalidMacName> for Diag {
    fn from(v: InvalidMacName) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("`{}` is not a valid macro name", v.lexeme),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: if v.is_name {
                    "cannot be used as a macro name"
                } else {
                    "must be an identifier"
                }
                .to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct NoSpaceAfterMacName {
    pub name: Name,
    pub first_span: Span,
}

impl From<NoSpaceAfterMacName> for Diag {
    fn from(v: NoSpaceAfterMacName) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!(
                "Missing whitespace between the name and body of macro `{}`",
                v.name
            ),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.first_span,
                text: "there must be whitespace before this token".to_owned(),
            }])],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MacParamListError {
    HasNewline(Span),
    ExpectedName(Span),
    DuplicateParam(Name, Span),
    EllipsisNotAtEnd(Span),
    ExpectedComma(Span),
}

#[derive(PartialEq, Debug)]
pub struct InvalidMacParamList(pub Vec<MacParamListError>);

impl From<InvalidMacParamList> for Diag {
    fn from(v: InvalidMacParamList) -> Self {
        let squiggles = v.0.into_iter().map(|e| {
            use MacParamListError::*;
            let (span, text) = match e {
                HasNewline(span) => (span, "cannot contain newlines".to_owned()),
                ExpectedName(span) => (span, "parameters can only be identifiers".to_owned()),
                DuplicateParam(name, span) => {
                    (span, format!("parameter `{}` is already defined", name))
                }
                EllipsisNotAtEnd(span) => (span, "must be the last parameter".to_owned()),
                ExpectedComma(span) => (span, "expected `,` or `)` instead of this".to_owned()),
            };
            Squiggle {
                primary: true,
                span,
                text,
            }
        });

        Diag {
            kind: DiagKind::Error,
            title: "Invalid macro parameter list".to_owned(),
            parts: vec![DiagPart::Snippet(squiggles.collect())],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct DirectiveInMacArgs {
    pub span: Span,
}

impl From<DirectiveInMacArgs> for Diag {
    fn from(v: DirectiveInMacArgs) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Unexpected directive".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "cannot appear inside macro arguments".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct WrongMacNumArgs {
    pub expected: usize,
    pub actual: usize,
    pub variadic: bool,
    pub defined_at: Option<Span>,
    pub args_span: Span,
    pub name: Name,
}

impl From<WrongMacNumArgs> for Diag {
    fn from(v: WrongMacNumArgs) -> Self {
        let mut parts = vec![DiagPart::Snippet(vec![Squiggle {
            primary: true,
            span: v.args_span,
            text: if v.variadic {
                // - 1 parce que dans une macro variadique on inclut toujours
                // l'arg variadique dans le nombre
                format!(
                    "found {} but expected at least {}",
                    v.actual - 1,
                    v.expected - 1
                )
            } else {
                format!("found {} but expected {}", v.actual, v.expected)
            },
        }])];

        if let Some(span) = v.defined_at {
            parts.push(DiagPart::Snippet(vec![Squiggle {
                primary: false,
                span,
                text: "macro defined here".to_owned(),
            }]))
        }

        let too = if v.actual > v.expected {
            "Too many"
        } else {
            "Too few"
        };
        Diag {
            kind: DiagKind::Error,
            title: format!("{too} arguments provided to macro `{}`", v.name),
            parts,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct UnterminatedMacCall {
    pub name: Name,
    pub span: Span,
}

impl From<UnterminatedMacCall> for Diag {
    fn from(v: UnterminatedMacCall) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("Unterminated call to macro `{}`", v.name),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "missing terminating `)`".to_owned(),
            }])],
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidStringize {
    pub stringize_span: Span,
    pub arg_span: Option<Span>,
    pub lexeme: String,
}

impl From<InvalidStringize> for Diag {
    fn from(v: InvalidStringize) -> Self {
        // todo: ça pourrait être bien d'afficher pourquoi la chaîne est invalide
        let mut parts = vec![DiagPart::Snippet(vec![Squiggle {
            primary: true,
            span: v.stringize_span,
            text: format!("formed the invalid string literal {}", v.lexeme),
        }])];
        if let Some(span) = v.arg_span {
            parts.push(DiagPart::Snippet(vec![Squiggle {
                primary: false,
                span,
                text: "from this argument".to_owned(),
            }]));
        }

        Diag {
            kind: DiagKind::Error,
            title: "Invalid stringize".to_owned(),
            parts,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct InvalidConcat<'a> {
    pub lhs_lexeme: &'a str,
    pub rhs_lexeme: &'a str,
    pub hash_hash_span: Span,
}

impl From<InvalidConcat<'_>> for Diag {
    fn from(v: InvalidConcat) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid concatenation".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.hash_hash_span,
                text: format!(
                    "cannot concatenate `{}` and `{}`",
                    v.lhs_lexeme, v.rhs_lexeme
                ),
            }])],
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct HashNotFollowedByParam {
    pub span: Span,
}

impl From<HashNotFollowedByParam> for Diag {
    fn from(v: HashNotFollowedByParam) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid use of the # operator".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "must be followed by a macro parameter".to_owned(),
            }])],
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct HashHashAtStartOrEnd {
    pub span: Span,
    pub at_start: bool,
    pub in_va_opt: bool,
}

impl From<HashHashAtStartOrEnd> for Diag {
    fn from(v: HashHashAtStartOrEnd) -> Self {
        let pos = if v.at_start { "start" } else { "end" };
        let body = if v.in_va_opt {
            "__VA_OPT__"
        } else {
            "macro body"
        };

        Diag {
            kind: DiagKind::Error,
            title: "Invalid use of the ## operator".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: format!("cannot appear at {pos} of {body}"),
            }])],
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct NestedVaOpt {
    pub span: Span,
}

impl From<NestedVaOpt> for Diag {
    fn from(v: NestedVaOpt) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: "Invalid use of `__VA_OPT__`".to_owned(),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "cannot be nested inside a `__VA_OPT__`".to_owned(),
            }])],
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ForbiddenHasExpr {
    pub name: Name,
    pub span: Span,
}

impl From<ForbiddenHasExpr> for Diag {
    fn from(v: ForbiddenHasExpr) -> Self {
        Diag {
            kind: DiagKind::Error,
            title: format!("Illegal use of `{}`", v.name),
            parts: vec![DiagPart::Snippet(vec![Squiggle {
                primary: true,
                span: v.span,
                text: "cannot appear outside a conditional directive".to_owned(),
            }])],
        }
    }
}
