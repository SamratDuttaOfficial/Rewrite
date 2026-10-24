//! Maths: TeX <-> Word equations (OMML), TeX -> MathML (LibreOffice), MathML -> TeX, and TeX as styled
//! text runs for pages drawn here (real superscripts and subscripts, Unicode symbols).
//!
//! The TeX understood is what equations in papers mostly use: letters and numbers, ^ and _, \frac, \sqrt,
//! Greek letters and the usual symbols, sums and integrals with limits, \left( ... \right), accents,
//! font commands (\mathbf, \mathrm, \text ...), matrices and aligned/cases environments.

use crate::xml::esc;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"), ("beta", "β"), ("gamma", "γ"), ("delta", "δ"), ("epsilon", "ϵ"), ("varepsilon", "ε"),
    ("zeta", "ζ"), ("eta", "η"), ("theta", "θ"), ("vartheta", "ϑ"), ("iota", "ι"), ("kappa", "κ"), ("lambda", "λ"),
    ("mu", "μ"), ("nu", "ν"), ("xi", "ξ"), ("pi", "π"), ("varpi", "ϖ"), ("rho", "ρ"), ("varrho", "ϱ"),
    ("sigma", "σ"), ("varsigma", "ς"), ("tau", "τ"), ("upsilon", "υ"), ("phi", "ϕ"), ("varphi", "φ"), ("chi", "χ"),
    ("psi", "ψ"), ("omega", "ω"), ("Gamma", "Γ"), ("Delta", "Δ"), ("Theta", "Θ"), ("Lambda", "Λ"), ("Xi", "Ξ"),
    ("Pi", "Π"), ("Sigma", "Σ"), ("Upsilon", "Υ"), ("Phi", "Φ"), ("Psi", "Ψ"), ("Omega", "Ω"),
    ("times", "×"), ("div", "÷"), ("cdot", "⋅"), ("pm", "±"), ("mp", "∓"), ("ast", "∗"), ("star", "⋆"),
    ("circ", "∘"), ("bullet", "∙"), ("oplus", "⊕"), ("ominus", "⊖"), ("otimes", "⊗"), ("odot", "⊙"), ("cap", "∩"),
    ("cup", "∪"), ("wedge", "∧"), ("land", "∧"), ("vee", "∨"), ("lor", "∨"), ("setminus", "∖"), ("leq", "≤"),
    ("le", "≤"), ("geq", "≥"), ("ge", "≥"), ("neq", "≠"), ("ne", "≠"), ("approx", "≈"), ("equiv", "≡"), ("sim", "∼"),
    ("simeq", "≃"), ("cong", "≅"), ("propto", "∝"), ("ll", "≪"), ("gg", "≫"), ("subset", "⊂"), ("supset", "⊃"),
    ("subseteq", "⊆"), ("supseteq", "⊇"), ("in", "∈"), ("notin", "∉"), ("ni", "∋"), ("perp", "⊥"), ("parallel", "∥"),
    ("mid", "∣"), ("models", "⊨"), ("vdash", "⊢"), ("prec", "≺"), ("succ", "≻"), ("preceq", "⪯"), ("succeq", "⪰"),
    ("doteq", "≐"), ("coloneqq", "≔"), ("to", "→"), ("rightarrow", "→"), ("leftarrow", "←"), ("gets", "←"),
    ("leftrightarrow", "↔"), ("Rightarrow", "⇒"), ("Leftarrow", "⇐"), ("Leftrightarrow", "⇔"), ("implies", "⟹"),
    ("iff", "⟺"), ("mapsto", "↦"), ("uparrow", "↑"), ("downarrow", "↓"), ("longrightarrow", "⟶"),
    ("longleftarrow", "⟵"), ("Longrightarrow", "⟹"), ("hookrightarrow", "↪"), ("nearrow", "↗"), ("searrow", "↘"),
    ("sum", "∑"), ("prod", "∏"), ("coprod", "∐"), ("int", "∫"), ("iint", "∬"), ("iiint", "∭"), ("oint", "∮"),
    ("bigcup", "⋃"), ("bigcap", "⋂"), ("bigoplus", "⨁"), ("bigotimes", "⨂"), ("bigvee", "⋁"), ("bigwedge", "⋀"),
    ("infty", "∞"), ("partial", "∂"), ("nabla", "∇"), ("forall", "∀"), ("exists", "∃"), ("nexists", "∄"),
    ("emptyset", "∅"), ("varnothing", "∅"), ("neg", "¬"), ("lnot", "¬"), ("angle", "∠"), ("triangle", "△"),
    ("ldots", "…"), ("cdots", "⋯"), ("vdots", "⋮"), ("ddots", "⋱"), ("dots", "…"), ("prime", "′"), ("hbar", "ℏ"),
    ("ell", "ℓ"), ("Re", "ℜ"), ("Im", "ℑ"), ("aleph", "ℵ"), ("wp", "℘"), ("top", "⊤"), ("bot", "⊥"),
    ("degree", "°"), ("langle", "⟨"), ("rangle", "⟩"), ("lceil", "⌈"), ("rceil", "⌉"), ("lfloor", "⌊"),
    ("rfloor", "⌋"), ("lvert", "|"), ("rvert", "|"), ("lVert", "‖"), ("rVert", "‖"), ("vert", "|"), ("Vert", "‖"),
    ("|", "‖"), ("backslash", "\\"), ("dagger", "†"), ("checkmark", "✓"), ("square", "□"), ("therefore", "∴"),
    ("because", "∵"), ("{", "{"), ("}", "}"), ("%", "%"), ("$", "$"), ("&", "&"), ("#", "#"), ("_", "_"),
    (",", "\u{2009}"), (";", "\u{2005}"), (":", "\u{2005}"), ("!", ""), ("quad", "\u{2003}"),
    ("qquad", "\u{2003}\u{2003}"), (" ", " "), ("colon", ":"),
];
const BIG: &[&str] = &["sum", "prod", "coprod", "int", "iint", "iiint", "oint", "bigcup", "bigcap", "bigoplus", "bigotimes", "bigvee", "bigwedge"];
pub const FUNCS: &[&str] = &["sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
                             "coth", "log", "ln", "lg", "exp", "lim", "liminf", "limsup", "max", "min", "sup", "inf", "arg",
                             "det", "dim", "gcd", "hom", "ker", "deg", "Pr", "argmax", "argmin", "mod", "bmod", "tr", "Tr",
                             "rank", "diag", "sgn"];
const ACCENTS: &[(&str, char)] = &[("hat", '\u{0302}'), ("widehat", '\u{0302}'), ("bar", '\u{0305}'), ("overline", '\u{0305}'),
                                   ("vec", '\u{20d7}'), ("tilde", '\u{0303}'), ("widetilde", '\u{0303}'), ("dot", '\u{0307}'),
                                   ("ddot", '\u{0308}'), ("check", '\u{030c}'), ("breve", '\u{0306}'), ("acute", '\u{0301}'),
                                   ("grave", '\u{0300}'), ("underline", '\u{0332}'), ("overrightarrow", '\u{20d7}')];
const STYLES: &[(&str, &str)] = &[("mathbf", "b"), ("boldsymbol", "b"), ("bm", "b"), ("mathrm", "p"), ("mathit", "i"),
                                  ("mathsf", "p"), ("mathtt", "p"), ("text", "t"), ("textrm", "t"), ("textit", "t"),
                                  ("textbf", "tb"), ("mbox", "t"), ("operatorname", "p"), ("mathbb", "dbl"),
                                  ("mathcal", "scr"), ("mathscr", "scr"), ("mathfrak", "frak")];

fn symbols() -> &'static HashMap<&'static str, &'static str> {
    static S: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    S.get_or_init(|| SYMBOLS.iter().copied().collect())
}

/// Unicode symbol -> TeX command name.
fn uni_to_tex() -> &'static HashMap<char, &'static str> {
    static S: OnceLock<HashMap<char, &str>> = OnceLock::new();
    S.get_or_init(|| {
        let mut m = HashMap::new();
        for (k, v) in SYMBOLS {
            let mut cs = v.chars();
            if let (Some(c), None) = (cs.next(), cs.next()) {
                if k.chars().all(|c| c.is_ascii_alphabetic()) {
                    m.entry(c).or_insert(*k);
                }
            }
        }
        for (c, k) in [('≤', "leq"), ('≥', "geq"), ('≠', "neq"), ('→', "to"), ('←', "leftarrow"), ('ε', "varepsilon"),
                       ('φ', "varphi"), ('∥', "parallel"), ('∣', "mid"), ('⋅', "cdot"), ('…', "ldots"), ('∧', "wedge"),
                       ('∨', "vee"), ('¬', "neg"), ('∅', "emptyset"), ('⊥', "perp")] {
            m.insert(c, k);
        }
        m
    })
}

fn accent_from(ch: char) -> &'static str {
    match ch {
        '\u{0302}' | '^' => "hat",
        '\u{0305}' | '¯' | '\u{0304}' => "bar",
        '\u{20d7}' | '→' | '\u{20d1}' => "vec",
        '\u{0303}' | '~' => "tilde",
        '\u{0307}' | '˙' => "dot",
        '\u{0308}' | '¨' => "ddot",
        '\u{030c}' | 'ˇ' => "check",
        '\u{0306}' => "breve",
        '\u{0301}' => "acute",
        '\u{0300}' => "grave",
        '\u{203e}' => "overline",
        _ => "hat",
    }
}

fn fence_cmd(t: &str) -> Option<&'static str> {
    Some(match t {
        "(" => "(", ")" => ")", "[" => "[", "]" => "]", "\\{" => "{", "\\}" => "}", "|" => "|", "\\|" => "‖", "." => "",
        "\\langle" => "⟨", "\\rangle" => "⟩", "\\lceil" => "⌈", "\\rceil" => "⌉", "\\lfloor" => "⌊", "\\rfloor" => "⌋",
        "\\lvert" => "|", "\\rvert" => "|", "\\lVert" => "‖", "\\rVert" => "‖", "\\vert" => "|", "\\Vert" => "‖",
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub enum Node {
    Row(Vec<Node>),
    /// text and kind: i = italic letter, n = number, o = operator, p = upright, t = text
    Sym(String, char),
    Sup(Box<Node>, Box<Node>),
    Sub(Box<Node>, Box<Node>),
    SubSup(Box<Node>, Box<Node>, Box<Node>),
    Frac(Box<Node>, Box<Node>),
    Sqrt(Box<Node>, Option<Box<Node>>),
    Nary(String, Option<Box<Node>>, Option<Box<Node>>, Option<Box<Node>>),
    Fence(String, String, Box<Node>),
    Acc(char, Box<Node>),
    Style(String, Box<Node>),
    Matrix(String, Vec<Vec<Node>>),
}

fn empty() -> Node {
    Node::Row(vec![])
}

struct Parser {
    toks: Vec<String>,
    i: usize,
}

impl Parser {
    fn new(tex: &str) -> Parser {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"(?s)\\[A-Za-z]+\*?|\\.|[{}^_&]|\s+|\d+(?:\.\d+)?|.").unwrap());
        Parser { toks: re.find_iter(tex.trim()).map(|m| m.as_str().to_string()).collect(), i: 0 }
    }

    fn peek(&mut self) -> Option<String> {
        while self.i < self.toks.len() && self.toks[self.i].trim().is_empty() {
            self.i += 1;
        }
        self.toks.get(self.i).cloned()
    }

    fn next(&mut self) -> Option<String> {
        let t = self.peek();
        self.i += 1;
        t
    }

    fn group(&mut self) -> Node {
        match self.peek().as_deref() {
            None => empty(),
            Some("{") => {
                self.next();
                let row = self.row(&["}"]);
                self.next();
                row
            }
            _ => self.atom().unwrap_or_else(empty),
        }
    }

    fn optional(&mut self) -> Option<Node> {
        if self.peek().as_deref() == Some("[") {
            self.next();
            let row = self.row(&["]"]);
            self.next();
            Some(row)
        } else {
            None
        }
    }

    fn raw_group(&mut self) -> String {
        if self.peek().as_deref() != Some("{") {
            return self.next().unwrap_or_default();
        }
        self.next();
        let mut depth = 1;
        let mut out = String::new();
        while self.i < self.toks.len() {
            let t = self.toks[self.i].clone();
            self.i += 1;
            if t == "{" {
                depth += 1;
            } else if t == "}" {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            out.push_str(&t);
        }
        out
    }

    fn row(&mut self, stops: &[&str]) -> Node {
        let mut items: Vec<Node> = vec![];
        loop {
            let Some(t) = self.peek() else { break };
            if stops.contains(&t.as_str()) || (t.starts_with('\\') && stops.contains(&&t[1..])) {
                break;
            }
            if t == "^" || t == "_" {
                self.next();
                let base = items.pop().unwrap_or_else(empty);
                let arg = Box::new(self.group());
                let node = match base {
                    Node::Nary(c, None, sup, body) if t == "_" => Node::Nary(c, Some(arg), sup, body),
                    Node::Nary(c, sub, None, body) if t == "^" => Node::Nary(c, sub, Some(arg), body),
                    Node::Sub(b, s) if t == "^" => Node::SubSup(b, s, arg),
                    Node::Sup(b, s) if t == "_" => Node::SubSup(b, arg, s),
                    other if t == "^" => Node::Sup(Box::new(other), arg),
                    other => Node::Sub(Box::new(other), arg),
                };
                items.push(node);
                continue;
            }
            if let Some(a) = self.atom() {
                items.push(a);
            }
        }
        // a big operator takes what follows as its body
        let mut out = vec![];
        let mut it = items.into_iter();
        while let Some(x) = it.next() {
            if let Node::Nary(c, sub, sup, None) = x {
                let rest: Vec<Node> = it.by_ref().collect();
                out.push(Node::Nary(c, sub, sup, Some(Box::new(Node::Row(rest)))));
                break;
            }
            out.push(x);
        }
        Node::Row(out)
    }

    fn atom(&mut self) -> Option<Node> {
        let t = self.next()?;
        if t == "{" {
            let row = self.row(&["}"]);
            self.next();
            return Some(row);
        }
        if t == "}" {
            return None;
        }
        if t == "&" {
            return Some(Node::Sym(String::new(), 'o'));
        }
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Some(Node::Sym(t, 'n'));
        }
        if !t.starts_with('\\') {
            let c = t.chars().next().unwrap();
            if c.is_alphabetic() {
                return Some(Node::Sym(t, 'i'));
            }
            return Some(Node::Sym(match c { '\'' => "′".into(), '~' => "\u{a0}".into(), _ => t }, 'o'));
        }
        let name = t[1..].to_string();
        let base = name.trim_end_matches('*');
        match base {
            "frac" | "dfrac" | "tfrac" | "cfrac" => return Some(Node::Frac(Box::new(self.group()), Box::new(self.group()))),
            "binom" | "tbinom" | "dbinom" => {
                let (a, b) = (self.group(), self.group());
                return Some(Node::Fence("(".into(), ")".into(), Box::new(Node::Matrix("binom".into(), vec![vec![a], vec![b]]))));
            }
            "sqrt" => {
                let idx = self.optional();
                return Some(Node::Sqrt(Box::new(self.group()), idx.map(Box::new)));
            }
            "left" => {
                let open = self.fence_char();
                let body = self.row(&["right"]);
                self.next();
                let close = self.fence_char();
                return Some(Node::Fence(open, close, Box::new(body)));
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" | "biggl" | "biggr" | "middle" => {
                return Some(Node::Sym(self.fence_char(), 'o'));
            }
            "begin" => {
                let env = self.raw_group().trim().to_string();
                if env == "array" || env == "tabular" {
                    self.raw_group();
                }
                let rows = self.matrix();
                return Some(Node::Matrix(env.trim_end_matches('*').to_string(), rows));
            }
            "limits" | "nolimits" | "displaystyle" | "textstyle" | "scriptstyle" | "nonumber" | "notag" | "mathstrut" => return None,
            "label" | "tag" | "hspace" | "vspace" | "phantom" => {
                self.raw_group();
                return None;
            }
            "overbrace" | "underbrace" | "boxed" => return Some(self.group()),
            "\\" => return Some(Node::Sym("\n".into(), 'o')),
            _ => {}
        }
        if BIG.contains(&base) {
            return Some(Node::Nary(symbols()[base].to_string(), None, None, None));
        }
        if FUNCS.contains(&base) {
            let text = match base { "argmax" => "arg max", "argmin" => "arg min", "bmod" => "mod", x => x };
            return Some(Node::Sym(text.into(), 'p'));
        }
        if let Some((_, ch)) = ACCENTS.iter().find(|(k, _)| *k == base) {
            return Some(Node::Acc(*ch, Box::new(self.group())));
        }
        if let Some((_, kind)) = STYLES.iter().find(|(k, _)| *k == base) {
            if *kind == "t" || *kind == "tb" {
                let raw = self.raw_group();
                return Some(Node::Style(kind.to_string(), Box::new(Node::Sym(raw, 't'))));
            }
            return Some(Node::Style(kind.to_string(), Box::new(self.group())));
        }
        if let Some(ch) = symbols().get(base) {
            let first = ch.chars().next();
            if first.is_some_and(|c| c.is_alphabetic()) {
                return Some(Node::Sym(ch.to_string(), if base.chars().next().unwrap().is_lowercase() { 'i' } else { 'p' }));
            }
            return Some(Node::Sym(ch.to_string(), 'o'));
        }
        Some(Node::Sym(base.to_string(), 'p'))
    }

    fn fence_char(&mut self) -> String {
        let t = self.next().unwrap_or_else(|| ".".into());
        if let Some(c) = fence_cmd(&t) {
            return c.to_string();
        }
        if let Some(s) = t.strip_prefix('\\') {
            if let Some(c) = symbols().get(s) {
                return c.to_string();
            }
        }
        t
    }

    fn matrix(&mut self) -> Vec<Vec<Node>> {
        let (mut rows, mut cells, mut cur): (Vec<Vec<Node>>, Vec<Node>, Vec<Node>) = (vec![], vec![], vec![]);
        loop {
            let Some(t) = self.peek() else { break };
            if t == "\\end" {
                self.next();
                self.raw_group();
                break;
            }
            if t == "&" {
                self.next();
                cells.push(Node::Row(std::mem::take(&mut cur)));
                continue;
            }
            if t == "\\\\" {
                self.next();
                if self.peek().as_deref() == Some("[") {
                    self.optional();
                }
                cells.push(Node::Row(std::mem::take(&mut cur)));
                rows.push(std::mem::take(&mut cells));
                continue;
            }
            if t == "^" || t == "_" {
                self.next();
                let base = cur.pop().unwrap_or_else(empty);
                let arg = self.group();
                cur.push(if t == "^" { Node::Sup(Box::new(base), Box::new(arg)) } else { Node::Sub(Box::new(base), Box::new(arg)) });
                continue;
            }
            if let Some(a) = self.atom() {
                cur.push(a);
            }
        }
        if !cur.is_empty() || !cells.is_empty() {
            cells.push(Node::Row(cur));
            rows.push(cells);
        }
        rows
    }
}

pub fn parse(tex: &str) -> Node {
    Parser::new(tex).row(&[])
}

// -- tree -> OMML -------------------------------------------------------------------

fn mr(text: &str, sty: Option<&str>) -> String {
    if text.is_empty() {
        return String::new();
    }
    let rpr = match sty {
        Some("t") => "<m:rPr><m:nor/></m:rPr>".to_string(),
        Some(s) => format!(r#"<m:rPr><m:sty m:val="{s}"/></m:rPr>"#),
        None => String::new(),
    };
    format!(r#"<m:r>{rpr}<m:t xml:space="preserve">{}</m:t></m:r>"#, esc(text))
}

fn omml(n: &Node, sty: Option<&str>) -> String {
    match n {
        Node::Row(items) => items.iter().map(|x| omml(x, sty)).collect(),
        Node::Sym(text, kind) => {
            if text == "\n" {
                String::new()
            } else if (*kind == 'p' || *kind == 'n') && sty.is_none() {
                mr(text, Some("p"))
            } else if *kind == 't' {
                mr(text, Some("t"))
            } else {
                mr(text, sty)
            }
        }
        Node::Sup(b, s) => format!("<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>", omml(b, sty), omml(s, sty)),
        Node::Sub(b, s) => format!("<m:sSub><m:e>{}</m:e><m:sub>{}</m:sub></m:sSub>", omml(b, sty), omml(s, sty)),
        Node::SubSup(b, d, u) => format!("<m:sSubSup><m:e>{}</m:e><m:sub>{}</m:sub><m:sup>{}</m:sup></m:sSubSup>", omml(b, sty), omml(d, sty), omml(u, sty)),
        Node::Frac(a, b) => format!("<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>", omml(a, sty), omml(b, sty)),
        Node::Sqrt(b, None) => format!(r#"<m:rad><m:radPr><m:degHide m:val="1"/></m:radPr><m:deg/><m:e>{}</m:e></m:rad>"#, omml(b, sty)),
        Node::Sqrt(b, Some(i)) => format!("<m:rad><m:deg>{}</m:deg><m:e>{}</m:e></m:rad>", omml(i, sty), omml(b, sty)),
        Node::Nary(ch, sub, sup, body) => {
            let loc = if "∫∬∭∮".contains(ch.as_str()) { "subSup" } else { "undOvr" };
            let mut props = format!(r#"<m:chr m:val="{ch}"/><m:limLoc m:val="{loc}"/>"#);
            if sub.is_none() {
                props.push_str(r#"<m:subHide m:val="1"/>"#);
            }
            if sup.is_none() {
                props.push_str(r#"<m:supHide m:val="1"/>"#);
            }
            let f = |x: &Option<Box<Node>>| x.as_ref().map(|n| omml(n, sty)).unwrap_or_default();
            format!("<m:nary><m:naryPr>{props}</m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary>", f(sub), f(sup), f(body))
        }
        Node::Fence(o, c, b) => format!(r#"<m:d><m:dPr><m:begChr m:val="{}"/><m:endChr m:val="{}"/></m:dPr><m:e>{}</m:e></m:d>"#, esc(o), esc(c), omml(b, sty)),
        Node::Acc(ch, b) => match ch {
            '\u{0305}' => format!(r#"<m:bar><m:barPr><m:pos m:val="top"/></m:barPr><m:e>{}</m:e></m:bar>"#, omml(b, sty)),
            '\u{0332}' => format!(r#"<m:bar><m:barPr><m:pos m:val="bot"/></m:barPr><m:e>{}</m:e></m:bar>"#, omml(b, sty)),
            _ => format!(r#"<m:acc><m:accPr><m:chr m:val="{ch}"/></m:accPr><m:e>{}</m:e></m:acc>"#, omml(b, sty)),
        },
        Node::Style(kind, b) => match kind.as_str() {
            "t" | "tb" => match b.as_ref() {
                Node::Sym(t, _) => mr(t, Some("t")),
                other => omml(other, sty),
            },
            "dbl" | "scr" | "frak" => {
                let scr = match kind.as_str() { "dbl" => "double-struck", "scr" => "script", _ => "fraktur" };
                let inner = omml(b, None);
                let re = Regex::new(r"<m:r>(<m:rPr>.*?</m:rPr>)?").unwrap();
                re.replace_all(&inner, format!(r#"<m:r><m:rPr><m:scr m:val="{scr}"/><m:sty m:val="p"/></m:rPr>"#).as_str()).into_owned()
            }
            "b" => omml(b, Some("b")),
            "p" => omml(b, Some("p")),
            "i" => omml(b, Some("i")),
            _ => omml(b, sty),
        },
        Node::Matrix(env, rows) => {
            if matches!(env.as_str(), "aligned" | "align" | "gathered" | "split" | "eqnarray" | "alignat" | "gather" | "multline") {
                let mut out = String::from("<m:eqArr>");
                for r in rows {
                    out.push_str("<m:e>");
                    for (j, c) in r.iter().enumerate() {
                        let mut x = omml(c, sty);
                        if j > 0 {
                            // an alignment point (&) is marked on the first run after it
                            if let Some(rest) = x.strip_prefix("<m:r><m:rPr>") {
                                x = format!("<m:r><m:rPr><m:aln/>{rest}");
                            } else if let Some(rest) = x.strip_prefix("<m:r>") {
                                x = format!("<m:r><m:rPr><m:aln/></m:rPr>{rest}");
                            }
                        }
                        out.push_str(&x);
                    }
                    out.push_str("</m:e>");
                }
                out.push_str("</m:eqArr>");
                return out;
            }
            if env == "cases" {
                let mut inner = String::from("<m:eqArr>");
                for r in rows {
                    inner.push_str("<m:e>");
                    for (j, c) in r.iter().enumerate() {
                        inner.push_str(&omml(c, sty));
                        if j + 1 < r.len() {
                            inner.push_str(&mr("\u{2003}", None));
                        }
                    }
                    inner.push_str("</m:e>");
                }
                inner.push_str("</m:eqArr>");
                return format!(r#"<m:d><m:dPr><m:begChr m:val="{{"/><m:endChr m:val=""/></m:dPr><m:e>{inner}</m:e></m:d>"#);
            }
            let body: String = std::iter::once("<m:m>".to_string())
                .chain(rows.iter().map(|r| format!("<m:mr>{}</m:mr>", r.iter().map(|c| format!("<m:e>{}</m:e>", omml(c, sty))).collect::<String>())))
                .chain(std::iter::once("</m:m>".to_string()))
                .collect();
            let fences = match env.as_str() {
                "pmatrix" => Some(("(", ")")),
                "bmatrix" => Some(("[", "]")),
                "Bmatrix" => Some(("{", "}")),
                "vmatrix" => Some(("|", "|")),
                "Vmatrix" => Some(("‖", "‖")),
                _ => None,
            };
            match fences {
                Some((o, c)) => format!(r#"<m:d><m:dPr><m:begChr m:val="{o}"/><m:endChr m:val="{c}"/></m:dPr><m:e>{body}</m:e></m:d>"#),
                None => body,
            }
        }
    }
}

/// An OMML equation (m:oMath, or m:oMathPara when display) for TeX source.
pub fn to_omml(tex: &str, display: bool) -> String {
    let math = format!("<m:oMath>{}</m:oMath>", omml(&parse(tex), None));
    if display {
        format!(r#"<m:oMathPara><m:oMathParaPr><m:jc m:val="center"/></m:oMathParaPr>{math}</m:oMathPara>"#)
    } else {
        math
    }
}

// -- OMML -> TeX --------------------------------------------------------------------

pub const M_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

fn nary_tex(ch: &str) -> String {
    match ch {
        "∑" => "\\sum", "∏" => "\\prod", "∐" => "\\coprod", "∫" => "\\int", "∬" => "\\iint", "∭" => "\\iiint",
        "∮" => "\\oint", "⋃" => "\\bigcup", "⋂" => "\\bigcap", "⨁" => "\\bigoplus", "⨂" => "\\bigotimes",
        "⋁" => "\\bigvee", "⋀" => "\\bigwedge",
        other => return other.to_string(),
    }
    .to_string()
}

fn fence_tex(ch: &str) -> String {
    match ch {
        "{" => "\\{", "}" => "\\}", "" => ".", "⟨" => "\\langle", "⟩" => "\\rangle", "‖" => "\\|", "⌈" => "\\lceil",
        "⌉" => "\\rceil", "⌊" => "\\lfloor", "⌋" => "\\rfloor",
        other => return other.to_string(),
    }
    .to_string()
}

pub fn tex_text(text: &str, upright: bool, normal: bool) -> String {
    if normal {
        return format!("\\text{{{}}}", text.replace('\\', "\\backslash ").replace('{', "\\{").replace('}', "\\}"));
    }
    let word = text.trim();
    if upright && FUNCS.contains(&word) {
        return format!("\\{word} ");
    }
    if upright && word.chars().count() > 1 && word.chars().all(|c| c.is_alphabetic()) {
        return format!("\\mathrm{{{word}}}");
    }
    let mut out = String::new();
    for ch in text.chars() {
        if let Some(k) = uni_to_tex().get(&ch) {
            out.push('\\');
            out.push_str(k);
            out.push(' ');
        } else {
            match ch {
                '{' | '}' | '%' | '$' | '#' | '&' | '_' => {
                    out.push('\\');
                    out.push(ch);
                }
                '\u{2009}' => out.push_str("\\,"),
                '\u{2005}' => out.push_str("\\;"),
                '\u{2003}' => out.push_str("\\quad "),
                '\u{a0}' => out.push('~'),
                '\\' => out.push_str("\\backslash "),
                c => out.push(c),
            }
        }
    }
    out
}

fn mchild<'a, 'i>(el: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    el.children().find(|n| n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(M_NS))
}

fn mval(el: roxmltree::Node, path: &[&str]) -> Option<String> {
    let mut cur = el;
    for p in path {
        cur = mchild(cur, p)?;
    }
    Some(cur.attribute((M_NS, "val")).unwrap_or("").to_string())
}

/// TeX source for an OMML element (m:oMath, m:oMathPara or any part of one).
pub fn omml_to_tex(el: roxmltree::Node) -> String {
    if !el.is_element() {
        return String::new();
    }
    let tag = el.tag_name().name();
    let kids = |e: Option<roxmltree::Node>| -> String { e.map(|e| e.children().map(omml_to_tex).collect()).unwrap_or_default() };
    let arg = |e: Option<roxmltree::Node>| -> String {
        let s = kids(e).trim().to_string();
        let single = s.chars().count() == 1;
        let cmd = s.starts_with('\\') && s.trim_end().chars().skip(1).all(|c| c.is_ascii_alphabetic());
        if single || cmd { s } else { format!("{{{s}}}") }
    };
    match tag {
        "r" => {
            let text: String = el.descendants().filter(|n| n.is_element() && n.tag_name().name() == "t").filter_map(|n| n.text()).collect();
            let rpr = mchild(el, "rPr");
            let sty = rpr.and_then(|r| mval(r, &["sty"]));
            let upright = matches!(sty.as_deref(), Some("p") | Some("b")) && !text.chars().all(|c| c.is_ascii_digit());
            let normal = rpr.is_some_and(|r| mchild(r, "nor").is_some());
            let mut s = tex_text(&text, upright, normal);
            if sty.as_deref() == Some("b") {
                s = format!("\\mathbf{{{}}}", s.trim());
            }
            match rpr.and_then(|r| mval(r, &["scr"])).as_deref() {
                Some("double-struck") => s = format!("\\mathbb{{{}}}", s.trim()),
                Some("script") => s = format!("\\mathcal{{{}}}", s.trim()),
                Some("fraktur") => s = format!("\\mathfrak{{{}}}", s.trim()),
                _ => {}
            }
            if rpr.is_some_and(|r| mchild(r, "aln").is_some()) {
                s = format!("&{s}");
            }
            s
        }
        "oMathPara" => el.children().filter(|n| n.is_element() && n.tag_name().name() == "oMath").map(omml_to_tex).collect::<Vec<_>>().join(" \\\\ "),
        "oMath" | "e" | "num" | "den" | "sub" | "sup" | "deg" | "fName" | "lim" => kids(Some(el)),
        "sSup" => format!("{}^{}", arg(mchild(el, "e")), arg(mchild(el, "sup"))),
        "sSub" => format!("{}_{}", arg(mchild(el, "e")), arg(mchild(el, "sub"))),
        "sSubSup" => format!("{}_{}^{}", arg(mchild(el, "e")), arg(mchild(el, "sub")), arg(mchild(el, "sup"))),
        "sPre" => format!("{{}}_{}^{}{}", arg(mchild(el, "sub")), arg(mchild(el, "sup")), arg(mchild(el, "e"))),
        "f" => match mval(el, &["fPr", "type"]).as_deref() {
            Some("lin") => format!("{}/{}", kids(mchild(el, "num")), kids(mchild(el, "den"))),
            Some("noBar") => format!("\\binom{{{}}}{{{}}}", kids(mchild(el, "num")), kids(mchild(el, "den"))),
            _ => format!("\\frac{{{}}}{{{}}}", kids(mchild(el, "num")), kids(mchild(el, "den"))),
        },
        "rad" => {
            let d = kids(mchild(el, "deg")).trim().to_string();
            let hide = matches!(mval(el, &["radPr", "degHide"]).as_deref(), Some("1") | Some("on") | Some("true"));
            format!("\\sqrt{}{{{}}}", if !d.is_empty() && !hide { format!("[{d}]") } else { String::new() }, kids(mchild(el, "e")))
        }
        "nary" => {
            let ch = mval(el, &["naryPr", "chr"]).filter(|c| !c.is_empty()).unwrap_or_else(|| "∫".into());
            let mut s = nary_tex(&ch);
            let sub = kids(mchild(el, "sub")).trim().to_string();
            let sup = kids(mchild(el, "sup")).trim().to_string();
            if !sub.is_empty() {
                s.push_str(&format!("_{{{sub}}}"));
            }
            if !sup.is_empty() {
                s.push_str(&format!("^{{{sup}}}"));
            }
            format!("{s} {}", kids(mchild(el, "e")))
        }
        "d" => {
            let pr = mchild(el, "dPr");
            let beg = pr.and_then(|p| mval(p, &["begChr"])).unwrap_or_else(|| "(".into());
            let end = pr.and_then(|p| mval(p, &["endChr"])).unwrap_or_else(|| ")".into());
            let sep = pr.and_then(|p| mval(p, &["sepChr"])).unwrap_or_else(|| "|".into());
            let inner = el.children().filter(|n| n.is_element() && n.tag_name().name() == "e").map(|e| kids(Some(e))).collect::<Vec<_>>()
                .join(&format!(" {} ", fence_tex(&sep)));
            format!("\\left{} {} \\right{}", fence_tex(&beg), inner, fence_tex(&end))
        }
        "func" => {
            let name = kids(mchild(el, "fName"));
            let name = name.trim_end();
            format!("{}{}{}", name, if name.starts_with('\\') { " " } else { "" }, arg(mchild(el, "e")))
        }
        "acc" => {
            let ch = mval(el, &["accPr", "chr"]).and_then(|c| c.chars().next()).unwrap_or('\u{0302}');
            format!("\\{}{{{}}}", accent_from(ch), kids(mchild(el, "e")))
        }
        "bar" => {
            let top = mval(el, &["barPr", "pos"]).as_deref() == Some("top");
            format!("{}{{{}}}", if top { "\\overline" } else { "\\underline" }, kids(mchild(el, "e")))
        }
        "limLow" => format!("{}_{}", arg(mchild(el, "e")), arg(mchild(el, "lim"))),
        "limUpp" => format!("{}^{}", arg(mchild(el, "e")), arg(mchild(el, "lim"))),
        "groupChr" => {
            let bot = mval(el, &["groupChrPr", "pos"]).as_deref() != Some("top");
            format!("{}{{{}}}", if bot { "\\underbrace" } else { "\\overbrace" }, kids(mchild(el, "e")))
        }
        "m" => {
            let rows: Vec<String> = el.children().filter(|n| n.is_element() && n.tag_name().name() == "mr")
                .map(|mr| mr.children().filter(|n| n.is_element() && n.tag_name().name() == "e").map(|e| kids(Some(e))).collect::<Vec<_>>().join(" & "))
                .collect();
            format!("\\begin{{matrix}} {} \\end{{matrix}}", rows.join(" \\\\ "))
        }
        "eqArr" => {
            let rows: Vec<String> = el.children().filter(|n| n.is_element() && n.tag_name().name() == "e").map(|e| kids(Some(e))).collect();
            format!("\\begin{{aligned}} {} \\end{{aligned}}", rows.join(" \\\\ "))
        }
        "box" | "borderBox" | "phant" => kids(mchild(el, "e")),
        t if t.ends_with("Pr") => String::new(),
        _ => kids(Some(el)),
    }
}

// -- tree -> MathML -----------------------------------------------------------------

fn mml(n: &Node) -> String {
    match n {
        Node::Row(items) => format!("<mrow>{}</mrow>", items.iter().map(mml).collect::<String>()),
        Node::Sym(text, kind) => {
            if text == "\n" || text.is_empty() {
                return String::new();
            }
            if *kind != 't' && text.chars().all(|c| c.is_whitespace()) {
                let em: f64 = text.chars().map(|c| match c { '\u{2009}' => 0.167, '\u{2005}' => 0.278, '\u{2003}' => 1.0, _ => 0.25 }).sum();
                return format!(r#"<mspace width="{em:.3}em"/>"#);
            }
            let tag = match kind { 'i' | 'p' => "mi", 'n' => "mn", 't' => "mtext", _ => "mo" };
            let attr = if *kind == 'p' || (*kind == 'i' && text.chars().count() > 1 && text.chars().all(|c| c.is_alphabetic())) {
                r#" mathvariant="normal""#
            } else {
                ""
            };
            format!("<{tag}{attr}>{}</{tag}>", esc(text))
        }
        Node::Sup(b, s) => format!("<msup>{}{}</msup>", mml(b), mml(s)),
        Node::Sub(b, s) => format!("<msub>{}{}</msub>", mml(b), mml(s)),
        Node::SubSup(b, d, u) => format!("<msubsup>{}{}{}</msubsup>", mml(b), mml(d), mml(u)),
        Node::Frac(a, b) => format!("<mfrac>{}{}</mfrac>", mml(a), mml(b)),
        Node::Sqrt(b, None) => format!("<msqrt>{}</msqrt>", mml(b)),
        Node::Sqrt(b, Some(i)) => format!("<mroot>{}{}</mroot>", mml(b), mml(i)),
        Node::Nary(ch, sub, sup, body) => {
            let op = format!("<mo>{ch}</mo>");
            let op = match (sub, sup) {
                (Some(d), Some(u)) => format!("<munderover>{op}{}{}</munderover>", mml(d), mml(u)),
                (Some(d), None) => format!("<munder>{op}{}</munder>", mml(d)),
                (None, Some(u)) => format!("<mover>{op}{}</mover>", mml(u)),
                _ => op,
            };
            format!("<mrow>{op}{}</mrow>", body.as_ref().map(|b| mml(b)).unwrap_or_default())
        }
        Node::Fence(o, c, b) => {
            let o = if o.is_empty() { String::new() } else { format!(r#"<mo fence="true">{}</mo>"#, esc(o)) };
            let c = if c.is_empty() { String::new() } else { format!(r#"<mo fence="true">{}</mo>"#, esc(c)) };
            format!("<mrow>{o}{}{c}</mrow>", mml(b))
        }
        Node::Acc(ch, b) => {
            if *ch == '\u{0332}' {
                return format!(r#"<munder accentunder="true">{}<mo>_</mo></munder>"#, mml(b));
            }
            let c = match ch {
                '\u{0302}' => "^", '\u{0305}' => "¯", '\u{20d7}' => "→", '\u{0303}' => "~", '\u{0307}' => "˙",
                '\u{0308}' => "¨", '\u{030c}' => "ˇ", '\u{0306}' => "˘", '\u{0301}' => "´", '\u{0300}' => "`", _ => "^",
            };
            format!(r#"<mover accent="true">{}<mo>{c}</mo></mover>"#, mml(b))
        }
        Node::Style(kind, b) => {
            if kind == "t" || kind == "tb" {
                if let Node::Sym(t, _) = b.as_ref() {
                    return format!("<mtext>{}</mtext>", esc(t));
                }
            }
            let v = match kind.as_str() { "b" => "bold", "i" => "italic", "dbl" => "double-struck", "scr" => "script", "frak" => "fraktur", _ => "normal" };
            format!(r#"<mstyle mathvariant="{v}">{}</mstyle>"#, mml(b))
        }
        Node::Matrix(env, rows) => {
            let table = format!("<mtable>{}</mtable>", rows.iter().map(|r| format!("<mtr>{}</mtr>", r.iter().map(|c| format!("<mtd>{}</mtd>", mml(c))).collect::<String>())).collect::<String>());
            let f = match env.as_str() {
                "pmatrix" => Some(("(", ")")), "bmatrix" => Some(("[", "]")), "Bmatrix" => Some(("{", "}")),
                "vmatrix" => Some(("|", "|")), "Vmatrix" => Some(("‖", "‖")), "cases" => Some(("{", "")), _ => None,
            };
            match f {
                Some((o, c)) => format!(r#"<mrow><mo fence="true">{o}</mo>{table}{}</mrow>"#, if c.is_empty() { String::new() } else { format!(r#"<mo fence="true">{c}</mo>"#) }),
                None => table,
            }
        }
    }
}

/// A MathML document for TeX source; the TeX is kept as an annotation so it can be read back exactly.
pub fn to_mathml(tex: &str, display: bool) -> String {
    format!(
        r#"<math xmlns="http://www.w3.org/1998/Math/MathML"{}><semantics>{}<annotation encoding="application/x-tex">{}</annotation></semantics></math>"#,
        if display { r#" display="block""# } else { "" }, mml(&parse(tex)), esc(tex))
}

pub const MML_NS: &str = "http://www.w3.org/1998/Math/MathML";

/// TeX for MathML (the TeX annotation when there is one).
pub fn mathml_to_tex(root: roxmltree::Node) -> String {
    for ann in root.descendants().filter(|n| n.is_element() && n.tag_name().name() == "annotation") {
        if matches!(ann.attribute("encoding"), Some("application/x-tex" | "TeX" | "LaTeX" | "application/x-latex")) {
            return ann.text().unwrap_or("").trim().to_string();
        }
    }
    fn br(s: &str) -> String {
        let s = s.trim();
        if s.chars().count() == 1 { s.to_string() } else { format!("{{{s}}}") }
    }
    fn conv(e: roxmltree::Node) -> String {
        if !e.is_element() {
            return String::new();
        }
        let tag = e.tag_name().name();
        let kids: Vec<roxmltree::Node> = e.children().filter(|c| c.is_element() && !matches!(c.tag_name().name(), "annotation" | "annotation-xml")).collect();
        let a: Vec<String> = kids.iter().map(|c| conv(*c)).collect();
        let variant = |v: Option<&str>| -> Option<&'static str> {
            Some(match v? {
                "bold" => "\\mathbf{", "double-struck" => "\\mathbb{", "script" => "\\mathcal{", "fraktur" => "\\mathfrak{",
                "bold-italic" => "\\boldsymbol{", _ => return None,
            })
        };
        match tag {
            "math" | "mrow" | "semantics" | "mstyle" | "mpadded" | "mphantom" | "menclose" | "mtd" => {
                let inner = a.concat();
                match (tag, variant(e.attribute("mathvariant"))) {
                    ("mstyle", Some(v)) => format!("{v}{inner}}}"),
                    _ => inner,
                }
            }
            "mi" | "mn" | "mo" => {
                let text = e.text().unwrap_or("").trim();
                let upright = tag == "mi" && (e.attribute("mathvariant") == Some("normal") || text.chars().count() > 1);
                let mut s = tex_text(text, upright, false);
                if tag == "mo" && "∑∏∐∫∬∭∮⋃⋂⨁⨂⋁⋀".contains(text) && !text.is_empty() {
                    s = nary_tex(text) + " ";
                }
                if let Some(v) = variant(e.attribute("mathvariant")) {
                    s = format!("{v}{}}}", s.trim());
                }
                s
            }
            "mtext" => {
                let t = e.text().unwrap_or("");
                if t.trim().chars().count() == 1 && !t.trim().chars().all(|c| c.is_alphanumeric()) {
                    tex_text(t.trim(), false, false) // LibreOffice writes "=" as text
                } else {
                    format!("\\text{{{t}}}")
                }
            }
            "mspace" => "\\,".into(),
            "msup" if a.len() == 2 => format!("{}^{}", br(&a[0]), br(&a[1])),
            "msub" if a.len() == 2 => format!("{}_{}", br(&a[0]), br(&a[1])),
            "msubsup" if a.len() == 3 => format!("{}_{}^{}", br(&a[0]), br(&a[1]), br(&a[2])),
            "munderover" if a.len() == 3 => format!("{}_{}^{} ", a[0].trim(), br(&a[1]), br(&a[2])),
            "munder" if a.len() == 2 => {
                if e.attribute("accentunder") == Some("true") { format!("\\underline{{{}}}", a[0]) } else { format!("{}_{} ", a[0].trim(), br(&a[1])) }
            }
            "mover" if a.len() == 2 => {
                if e.attribute("accent") == Some("true") {
                    let ch = kids[1].text().unwrap_or("^").trim().chars().next().unwrap_or('^');
                    format!("\\{}{{{}}}", accent_from(ch), a[0])
                } else {
                    format!("{}^{} ", a[0].trim(), br(&a[1]))
                }
            }
            "mfrac" if a.len() == 2 => format!("\\frac{{{}}}{{{}}}", a[0], a[1]),
            "msqrt" => format!("\\sqrt{{{}}}", a.concat()),
            "mroot" if a.len() == 2 => format!("\\sqrt[{}]{{{}}}", a[1], a[0]),
            "mfenced" => format!("\\left{} {} \\right{}", fence_tex(e.attribute("open").unwrap_or("(")), a.join(", "), fence_tex(e.attribute("close").unwrap_or(")"))),
            "mtable" => format!("\\begin{{matrix}} {} \\end{{matrix}}", a.join(" \\\\ ")),
            "mtr" => a.join(" & "),
            _ => a.concat(),
        }
    }
    conv(root).trim().to_string()
}

pub fn mathml_xml_to_tex(xml: &str) -> String {
    roxmltree::Document::parse(xml).map(|d| mathml_to_tex(d.root_element())).unwrap_or_default()
}

// -- tree -> styled text (for pages drawn here) ------------------------------------------

/// The maths as [(text, level, italic, bold)], level +1 = superscript, -1 = subscript.
pub fn runs(tex: &str) -> Vec<(String, i32, bool, bool)> {
    let mut out: Vec<(String, i32, bool, bool)> = vec![];
    fn emit(out: &mut Vec<(String, i32, bool, bool)>, t: &str, level: i32, italic: bool, bold: bool) {
        if !t.is_empty() {
            out.push((t.to_string(), level, italic, bold));
        }
    }
    fn walk(n: &Node, level: i32, bold: bool, upright: bool, out: &mut Vec<(String, i32, bool, bool)>) {
        match n {
            Node::Row(items) => items.iter().for_each(|x| walk(x, level, bold, upright, out)),
            Node::Sym(text, kind) => {
                if text == "\n" {
                    emit(out, "; ", level, false, false);
                    return;
                }
                let italic = *kind == 'i' && !upright && text.chars().count() == 1 && text.chars().all(|c| c.is_alphabetic());
                let mut t = text.clone();
                if level == 0 && *kind == 'o' && "=<>≤≥≠≈≡→⇒⇔∈∉⊂⊆∼+−-±×".contains(text.as_str()) && text.chars().count() == 1 {
                    t = format!(" {text} ");
                } else if *kind == 'p' && FUNCS.contains(&text.as_str()) {
                    t = format!("{text} ");
                }
                emit(out, &t, level, italic, bold);
            }
            Node::Sup(base, _) | Node::Sub(base, _) | Node::SubSup(base, _, _) => {
                let func = matches!(base.as_ref(), Node::Sym(t, 'p') if FUNCS.contains(&t.as_str()));
                if let (true, Node::Sym(t, _)) = (func, base.as_ref()) {
                    emit(out, t, level, false, bold);
                } else {
                    walk(base, level, bold, upright, out);
                }
                match n {
                    Node::Sup(_, s) => walk(s, if level < 1 { level + 1 } else { level }, bold, upright, out),
                    Node::Sub(_, s) => walk(s, if level > -1 { level - 1 } else { level }, bold, upright, out),
                    Node::SubSup(_, d, u) => {
                        walk(d, -1, bold, upright, out);
                        walk(u, 1, bold, upright, out);
                    }
                    _ => {}
                }
                if func {
                    emit(out, " ", level, false, false);
                }
            }
            Node::Frac(a, b) => {
                let simple = |x: &Node| match x {
                    Node::Row(v) => v.iter().all(|y| matches!(y, Node::Sym(..))),
                    Node::Sym(..) => true,
                    _ => false,
                };
                let s = simple(a) && simple(b);
                if !s {
                    emit(out, "(", level, false, false);
                }
                walk(a, level, bold, upright, out);
                emit(out, if s { "/" } else { ")/(" }, level, false, false);
                walk(b, level, bold, upright, out);
                if !s {
                    emit(out, ")", level, false, false);
                }
            }
            Node::Sqrt(b, idx) => {
                if let Some(i) = idx {
                    walk(i, 1, bold, upright, out);
                }
                emit(out, "√(", level, false, false);
                walk(b, level, bold, upright, out);
                emit(out, ")", level, false, false);
            }
            Node::Nary(ch, sub, sup, body) => {
                emit(out, ch, level, false, false);
                if let Some(d) = sub {
                    walk(d, -1, bold, upright, out);
                }
                if let Some(u) = sup {
                    walk(u, 1, bold, upright, out);
                }
                emit(out, " ", level, false, false);
                if let Some(b) = body {
                    walk(b, level, bold, upright, out);
                }
            }
            Node::Fence(o, c, b) => {
                emit(out, o, level, false, false);
                walk(b, level, bold, upright, out);
                emit(out, c, level, false, false);
            }
            Node::Acc(ch, b) => {
                let start = out.len();
                walk(b, level, bold, upright, out);
                if out.len() > start {
                    out.last_mut().unwrap().0.push(*ch);
                }
            }
            Node::Style(kind, b) => match kind.as_str() {
                "t" | "tb" => {
                    if let Node::Sym(t, _) = b.as_ref() {
                        emit(out, t, level, false, kind == "tb");
                    }
                }
                "dbl" => {
                    let start = out.len();
                    walk(b, level, bold, true, out);
                    for item in out.iter_mut().skip(start) {
                        item.0 = item.0.chars().map(|c| match c {
                            'R' => 'ℝ', 'N' => 'ℕ', 'Z' => 'ℤ', 'Q' => 'ℚ', 'C' => 'ℂ', 'P' => 'ℙ', 'H' => 'ℍ', x => x,
                        }).collect();
                        item.2 = false;
                    }
                }
                k => walk(b, level, k == "b" || bold, k == "p" || k == "b" || upright, out),
            },
            Node::Matrix(env, rows) => {
                let (o, c) = match env.as_str() {
                    "pmatrix" => ("(", ")"), "bmatrix" => ("[", "]"), "Bmatrix" => ("{", "}"), "vmatrix" => ("|", "|"),
                    "Vmatrix" => ("‖", "‖"), "cases" => ("{ ", ""), _ => ("", ""),
                };
                emit(out, o, level, false, false);
                for (r, row) in rows.iter().enumerate() {
                    if r > 0 {
                        emit(out, "; ", level, false, false);
                    }
                    for (k, cell) in row.iter().enumerate() {
                        if k > 0 {
                            emit(out, if matches!(env.as_str(), "aligned" | "align" | "cases" | "split") { "  " } else { ", " }, level, false, false);
                        }
                        walk(cell, level, bold, upright, out);
                    }
                }
                emit(out, c, level, false, false);
            }
        }
    }
    walk(&parse(tex), 0, false, false, &mut out);
    let mut merged: Vec<(String, i32, bool, bool)> = vec![];
    for x in out {
        if let Some(last) = merged.last_mut() {
            if (last.1, last.2, last.3) == (x.1, x.2, x.3) {
                last.0.push_str(&x.0);
                continue;
            }
        }
        merged.push(x);
    }
    merged
}

/// A one-line Unicode version of the maths (for plain text).
pub fn to_unicode(tex: &str) -> String {
    const SUP: &str = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿⁱ";
    const SUB: &str = "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎";
    let map = |t: &str, table: &str, from: &str| -> Option<String> {
        let tbl: Vec<char> = table.chars().collect();
        t.chars().map(|c| from.find(c).map(|i| tbl[from[..i].chars().count()])).collect()
    };
    // neighbouring pieces at the same level are converted together (x_{i=1} is one subscript)
    let mut grouped: Vec<(String, i32)> = vec![];
    for (text, level, _, _) in runs(tex) {
        match grouped.last_mut() {
            Some(last) if last.1 == level => last.0.push_str(&text),
            _ => grouped.push((text, level)),
        }
    }
    let mut out = String::new();
    for (text, level) in grouped {
        if level > 0 {
            match map(&text, SUP, "0123456789+-=()ni") {
                Some(s) => out.push_str(&s),
                None => out.push_str(&format!("^({text})")),
            }
        } else if level < 0 {
            match map(&text, SUB, "0123456789+-=()") {
                Some(s) => out.push_str(&s),
                None => out.push_str(&format!("_({text})")),
            }
        } else {
            out.push_str(&text);
        }
    }
    let re = Regex::new(" {2,}").unwrap();
    re.replace_all(out.trim(), " ").into_owned()
}
