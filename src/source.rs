//! ~ source management ~

use crate::{lex::eat_newline, name::Name};
use std::{
    bstr::{ByteStr, ByteString},
    cell::{Cell, OnceCell},
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SourceId(u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SubstId(u32);

pub struct Source {
    id: SourceId,
    path: PathBuf,
    text: String,
    start: Loc,
    line_remaps: Vec<LineRemap>,
    /// stocke l'offset du début de chaque ligne (la ligne 1 est à l'indice 0)
    line_starts: OnceCell<Vec<u32>>,
}

impl Source {
    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn start(&self) -> Loc {
        self.start
    }

    pub fn line_starts(&self) -> &Vec<u32> {
        self.line_starts
            .get_or_init(|| find_line_starts(&self.text))
    }
}

/// représente un remapping introduit par une directive #line
/// (indique qu'à partir de tel offset, on se trouve sur la ligne line et fichier file_name)
// todo: interner le filename?
pub struct LineRemap {
    pub offset: Loc,
    pub line: u32,
    pub file_name: Option<ByteString>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LocOrigin {
    Source(SourceId),
    Subst(SubstId),
}

#[derive(Clone, Copy, Debug)]
struct LocMapEntry {
    origin: LocOrigin,
    span: Span,
}

pub struct SourceHub {
    loc_map: Vec<LocMapEntry>,
    last_entry: Cell<Option<LocMapEntry>>,
    sources: Vec<Source>,
    substs: Vec<Subst>,
    next_loc: Loc,
}

impl SourceHub {
    pub fn new() -> Self {
        Self {
            loc_map: Vec::new(),
            sources: Vec::new(),
            substs: Vec::new(),
            next_loc: Loc(0),
            last_entry: Cell::new(None),
        }
    }

    pub fn source(&self, id: SourceId) -> &Source {
        &self.sources[id.0 as usize]
    }

    pub fn source_from_loc(&self, loc: Loc) -> &Source {
        match self.loc_origin(loc) {
            LocOrigin::Source(id) => self.source(id),
            _ => panic!("loc doit venir d'une source"),
        }
    }

    pub fn loc_origin(&self, loc: Loc) -> LocOrigin {
        if let Some(entry) = self.last_entry.get()
            && entry.span.contains(loc)
        {
            return entry.origin;
        }

        let i = match self.loc_map.binary_search_by(|e| e.span.lo.cmp(&loc)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };

        let entry = self.loc_map[i];
        self.last_entry.set(Some(entry));
        entry.origin
    }

    pub fn span_origin(&self, span: Span) -> LocOrigin {
        debug_assert!(self.is_valid(span));
        self.loc_origin(span.lo)
    }

    /// retourne le span correspondant à une source (si le span vient d'une
    /// substitution, on "traverse" la substitution pour changer "d'espace",
    /// jusqu'à tomber sur une source)
    #[must_use]
    pub fn source_span(&self, span: Span) -> Span {
        match self.span_origin(span) {
            LocOrigin::Source(_) => span,
            LocOrigin::Subst(id) => {
                let subst = self.subst(id);
                self.source_span(Span {
                    lo: span.lo.remove_subst(subst),
                    hi: span.hi.remove_subst(subst),
                })
            }
        }
    }

    #[must_use]
    pub fn walk_up_to_source(&self, mut span: Span) -> Span {
        loop {
            match self.span_origin(span) {
                LocOrigin::Source(_) => return span,
                LocOrigin::Subst(id) => {
                    let subst = self.subst(id);
                    // si on tombe sur un include il faut "changer d'espace"
                    // pour se placer dans celui qui correspond à la source qui
                    // a été includée
                    // si on se contentait de remonter les substitutions jusqu'à
                    // la source on remonterait systématiquement jusqu'au fichier
                    // principal
                    match subst.kind {
                        SubstKind::Include => return self.source_span(span),
                        _ => span = subst.src,
                    }
                }
            }
        }
    }

    /// remonte les substitutions jusqu'à la source principale
    #[must_use]
    pub fn walk_up_to_base_source(&self, span: Span) -> Span {
        self.walk_up_until(span, |o| matches!(o, LocOrigin::Source(_)))
            .expect("il doit y avoir une source de base")
    }

    /// retourne le texte délimité par `span`
    pub fn text(&self, span: Span) -> &str {
        debug_assert!(self.is_valid(span));
        let source = self.source_from_loc(span.lo);
        &source.text[(span.lo.0 - source.start.0) as usize..(span.hi.0 - source.start.0) as usize]
    }

    pub fn subst(&self, id: SubstId) -> &Subst {
        &self.substs[id.0 as usize]
    }

    pub fn add_source(&mut self, path: PathBuf, text: String) -> &Source {
        let span = self.alloc_span(text.len() as u32);
        let id = SourceId(self.sources.len() as u32);

        self.sources.push(Source {
            id,
            start: span.lo,
            line_remaps: Vec::new(),
            line_starts: OnceCell::new(),
            path,
            text,
        });
        self.loc_map.push(LocMapEntry {
            origin: LocOrigin::Source(id),
            span,
        });

        self.sources.last().unwrap()
    }

    pub fn add_subst(&mut self, kind: SubstKind, src: Span, dst: Span) -> &Subst {
        debug_assert!(self.is_valid(src));
        debug_assert!(self.is_valid(dst));

        let span = self.alloc_span(dst.len());
        let id = SubstId(self.substs.len() as u32);

        self.substs.push(Subst {
            kind,
            src,
            dst,
            start: span.lo,
        });
        self.loc_map.push(LocMapEntry {
            origin: LocOrigin::Subst(id),
            span,
        });

        self.substs.last().unwrap()
    }

    pub fn add_line_remap(&mut self, id: SourceId, mut remap: LineRemap) {
        let source = &mut self.sources[id.0 as usize];
        if let Some(last) = source.line_remaps.last() {
            debug_assert!(
                remap.offset.0 > last.offset.0,
                "les remaps doivent être ajoutés par ordre croissant des offsets"
            );
            if remap.file_name.is_none() {
                // on hérite du filename précédent pour éviter de devoir aller le
                // chercher "en arrière" au moment de récupérer la presumed loc
                remap.file_name = last.file_name.clone();
            }
        }

        source.line_remaps.push(remap);
    }

    pub fn presumed_full_loc(&'_ self, loc: Loc) -> FullLoc<'_> {
        let source = self.source_from_loc(loc);
        let file_name = FileName::Path(source.path());
        let line = find_line(source, loc);
        let col = find_col(source, loc);

        if let Some(remap) = find_line_remap(source, loc) {
            return FullLoc {
                file_name: remap
                    .file_name
                    .as_ref()
                    .map(|f| FileName::Str(ByteStr::new(f.as_slice())))
                    .unwrap_or(file_name),
                // - 1 parce que l'offset du line remap se trouve à la fin de la ligne
                // `#line blabla` donc il y a un newline de trop, il faut pas le compter
                line: remap.line + line - find_line(source, remap.offset) - 1,
                col,
            };
        }

        FullLoc {
            file_name,
            line,
            col,
        }
    }

    pub fn write_virtual_source(&mut self, text: &str) -> Loc {
        // todo: au lieu de recréer une source à chaque fois, il vaudrait mieux
        // réutiliser la même et en créer une nouvelle que si elle est pleine
        self.add_source("<virtual source>".into(), text.to_owned())
            .start
    }

    /// retourne un span correspondant à l'union de a et b
    /// (si il n'ont pas la même origine ils sont alignés sur la même origine)
    pub fn merge(&self, a: Span, b: Span) -> Span {
        debug_assert!(self.is_valid(a));
        debug_assert!(self.is_valid(b));
        let (a, b) = self.align_to_common_origin(a, b);
        let span = Span {
            lo: a.lo.min(b.lo),
            hi: a.hi.max(b.hi),
        };
        debug_assert!(self.is_valid(span));
        span
    }

    /// remonte la chaîne d'origines jusqu'à trouver une origine qui vérifie le
    /// prédicat
    pub fn walk_up_until(&self, mut span: Span, pred: impl Fn(LocOrigin) -> bool) -> Option<Span> {
        loop {
            let origin = self.span_origin(span);
            if pred(origin) {
                return Some(span);
            }
            match origin {
                LocOrigin::Source(_) => return None,
                LocOrigin::Subst(id) => span = self.subst(id).src,
            }
        }
    }

    /// remonte les origines jusqu'à trouver 2 origines qui vérifient le prédicat
    /// (on compare chaque origine à partir de `a` à chaque origine à partir de `b`)
    ///
    /// retourne les spans correspondant aux origines qui vérifient le prédicat
    pub fn walk_up_pairwise_until(
        &self,
        a: Span,
        b: Span,
        pred: impl Fn(LocOrigin, LocOrigin) -> bool,
    ) -> Option<(Span, Span)> {
        let mut a_origin = self.span_origin(a);
        let mut b_origin = self.span_origin(b);
        let mut a_span = a;
        let mut b_span = b;
        let b_origin_save = b_origin;
        let b_span_save = b_span;

        loop {
            if pred(a_origin, b_origin) {
                return Some((a_span, b_span));
            }

            match b_origin {
                LocOrigin::Source(_) => {
                    // on a remonté `b` jusqu'à la source, on peut pas aller plus haut
                    // donc on remonte `a` d'un niveau et on recommence
                    b_origin = b_origin_save;
                    b_span = b_span_save;

                    match a_origin {
                        LocOrigin::Source(_) => return None,
                        LocOrigin::Subst(id) => {
                            a_span = self.subst(id).src;
                            a_origin = self.span_origin(a_span);
                        }
                    }
                }
                LocOrigin::Subst(id) => {
                    b_span = self.subst(id).src;
                    b_origin = self.span_origin(b_span);
                }
            }
        }
    }

    /// retourne les spans "alignés" sur l'origine commune la plus proche
    pub fn align_to_common_origin(&self, a: Span, b: Span) -> (Span, Span) {
        self.walk_up_pairwise_until(a, b, |a_origin, b_origin| a_origin == b_origin)
            .expect("il doit y avoir une origine commune")
    }

    /// retourne la "profondeur" d'une Loc, c'est-à-dire le nombre de substitutions
    /// appliquées
    pub fn depth(&self, mut loc: Loc) -> u32 {
        let mut i = 0;
        loop {
            match self.loc_origin(loc) {
                LocOrigin::Source(_) => return i,
                LocOrigin::Subst(id) => {
                    loc = self.subst(id).src.lo;
                    i += 1;
                }
            }
        }
    }

    /// retourne la profondeur d'inclusion
    pub fn include_depth(&self, mut loc: Loc) -> u32 {
        let mut i = 0;
        loop {
            match self.loc_origin(loc) {
                LocOrigin::Source(_) => return i,
                LocOrigin::Subst(id) => {
                    let subst = self.subst(id);
                    if subst.kind == SubstKind::Include {
                        i += 1;
                    }
                    loc = subst.src.lo;
                }
            }
        }
    }

    pub fn is_valid(&self, span: Span) -> bool {
        // chaque Loc couverte par ce span doit provenir de la même origine
        let origin = self.loc_origin(span.lo);
        span.lo.0 <= span.hi.0
            && (span.lo.0 + 1..span.hi.0).all(|i| self.loc_origin(Loc(i)) == origin)
    }

    fn alloc_span(&mut self, len: u32) -> Span {
        let span = Span {
            lo: self.next_loc,
            // + 1 parce que le hi d'un span est exclusif et on veut qu'une Loc
            // qui pointe sur `len` soit considérée comme faisant partie de cette
            // source et pas la suivante
            hi: Loc(self.next_loc.0 + len + 1),
        };
        self.next_loc = span.hi;
        span
    }

    /// fonction utilisée juste par les tests
    pub fn set_next_loc(&mut self, loc: Loc) {
        self.next_loc = loc;
    }
}

impl Default for SourceHub {
    fn default() -> Self {
        Self::new()
    }
}

fn find_col(source: &Source, loc: Loc) -> u32 {
    let end = (loc.0 - source.start.0) as usize;
    source.text.as_bytes()[..end]
        .iter()
        .rev()
        .position(|b| matches!(b, b'\r' | b'\n'))
        .unwrap_or(end) as u32
        + 1
}

fn find_line(source: &Source, loc: Loc) -> u32 {
    let offset = loc.0 - source.start.0;
    match source
        .line_starts()
        .binary_search_by(|start| start.cmp(&offset))
    {
        Ok(i) => i as u32 + 1,
        Err(i) => i as u32,
    }
}

fn find_line_remap(source: &Source, loc: Loc) -> Option<&LineRemap> {
    match source.line_remaps.binary_search_by(|r| r.offset.cmp(&loc)) {
        Ok(i) => Some(&source.line_remaps[i]),
        Err(0) => None,
        Err(i) => Some(&source.line_remaps[i - 1]),
    }
}

fn find_line_starts(src: &str) -> Vec<u32> {
    // todo: ça serait mieux d'itérer sur des bytes
    let mut it = src.chars();
    let mut line_starts = vec![0];

    loop {
        if eat_newline(&mut it) {
            line_starts.push((src.len() - it.as_str().len()) as u32);
        } else if it.next().is_none() {
            break;
        }
    }

    debug_assert!(line_starts.is_sorted());
    line_starts
}

#[derive(Clone, PartialEq)]
pub enum SubstKind {
    Include,
    MacExpansion { name: Name, name_span: Option<Span> },
    Other,
}

#[derive(Clone)]
pub struct Subst {
    pub kind: SubstKind,
    /// le span original qui se fait substituer
    pub src: Span,
    /// le nouveau span qui pointe vers le contenu substitué (body de macro, etc)
    pub dst: Span,
    pub start: Loc,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Loc(pub u32);

impl Loc {
    #[must_use]
    pub fn apply_subst(self, subst: &Subst) -> Loc {
        Loc(self.0 + subst.start.0 - subst.dst.lo.0)
    }

    #[must_use]
    fn remove_subst(self, subst: &Subst) -> Loc {
        Loc(self.0 - subst.start.0 + subst.dst.lo.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Span {
    // todo: c'est public mais c'est dangereux de modifier des spans, il faudrait
    // peut-être imposer de passer par le source hub
    pub lo: Loc,
    pub hi: Loc,
}

impl Span {
    #[must_use]
    pub fn apply_subst(self, subst: &Subst) -> Self {
        Self {
            lo: self.lo.apply_subst(subst),
            hi: self.hi.apply_subst(subst),
        }
    }

    pub fn contains(&self, loc: Loc) -> bool {
        self.lo <= loc && loc < self.hi
    }

    pub fn len(&self) -> u32 {
        self.hi.0 - self.lo.0
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub enum FileName<'a> {
    Path(&'a Path),
    Str(&'a ByteStr),
}

pub struct FullLoc<'a> {
    pub file_name: FileName<'a>,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FileStatus {
    Available,
    Invalid(LoadError),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LoadError {
    NotFound,
    Unreadable,
}

// todo: il faudrait que ça puisse retourner + d'infos sur le fichier, par ex
// le timestamp et de quoi établir son identité (pour #pragma once)
pub trait FileLoader {
    fn load(&self, path: &Path) -> Result<Vec<u8>, LoadError>;

    // il y a une implémentation par défaut mais il faut mieux l'override pour
    // éviter de charger le fichier pour rien
    fn status(&self, path: &Path) -> FileStatus {
        match self.load(path) {
            Ok(_) => FileStatus::Available,
            Err(e) => FileStatus::Invalid(e),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(e: io::Error) -> LoadError {
        match e.kind() {
            ErrorKind::NotFound | ErrorKind::InvalidFilename => LoadError::NotFound,
            _ => LoadError::Unreadable,
        }
    }
}

#[derive(Clone, Copy)]
pub struct FsFileLoader;

impl FileLoader for FsFileLoader {
    fn load(&self, path: &Path) -> Result<Vec<u8>, LoadError> {
        Ok(fs::read(path)?)
    }

    fn status(&self, path: &Path) -> FileStatus {
        match fs::File::open(path) {
            Ok(_) => FileStatus::Available,
            Err(e) => FileStatus::Invalid(e.into()),
        }
    }
}
