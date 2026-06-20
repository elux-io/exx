//! ~ diagnostics (errors, warnings, etc) ~

use crate::source::Span;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagKind {
    Error,
    Warn,
    Info,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Diag {
    pub kind: DiagKind,
    pub title: String,
    pub parts: Vec<DiagPart>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum DiagPart {
    Text(String),
    Snippet(Vec<Squiggle>),
    Diag(Diag),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Squiggle {
    pub primary: bool,
    pub span: Span,
    pub text: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Diags {
    diags: Vec<Diag>,
}

impl Diags {
    pub fn new() -> Self {
        Diags { diags: Vec::new() }
    }

    pub fn diags(&self) -> &Vec<Diag> {
        &self.diags
    }

    // todo: est-ce qu'on voudrait pas #[inline(never)] pour éviter de polluer
    // les hot paths avec de la gestion d'erreur ?
    pub fn emit(&mut self, diag: impl Into<Diag>) {
        self.diags.push(diag.into());
    }
}
