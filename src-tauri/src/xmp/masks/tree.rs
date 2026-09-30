//! Minimal span-preserving XML element tree for the mask reader/writer. `quick-xml` only
//! tokenizes (and validates); every element keeps the byte spans of its start tag, content
//! and attributes so the writer can copy untouched items verbatim and splice the rest.

use std::collections::HashMap;

use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::Reader;

pub const NS_RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";

/// One attribute of a start tag.
#[derive(Debug, Clone)]
pub struct Attr {
    /// Prefix as written (`""` = none).
    pub prefix: String,
    pub local: String,
    /// Resolved namespace (`""` for unprefixed attributes and `xmlns` declarations).
    pub uri: String,
    /// Unescaped value.
    pub value: String,
    /// Start of the whitespace before the attribute name.
    pub ws_start: usize,
    pub name_start: usize,
    /// After the closing quote.
    pub end: usize,
}

impl Attr {
    pub fn is_xmlns(&self) -> bool {
        self.prefix == "xmlns" || (self.prefix.is_empty() && self.local == "xmlns")
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub prefix: String,
    pub local: String,
    pub uri: String,
    /// `<` of the start tag.
    pub start: usize,
    /// After `>` of the start tag (= `end` for empty elements).
    pub open_end: usize,
    /// `<` of the end tag (= `end` for empty elements).
    pub close_start: usize,
    /// After the element.
    pub end: usize,
    pub attrs: Vec<Attr>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    /// Unescaped character data directly inside the element.
    pub text: String,
    /// Namespace bindings in scope inside the element (prefix -> uri).
    pub scope: HashMap<String, String>,
}

impl Node {
    pub fn is(&self, uri: &str, local: &str) -> bool {
        self.uri == uri && self.local == local
    }

    pub fn attr(&self, uri: &str, local: &str) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.uri == uri && a.local == local)
    }

    /// Prefix bound to `uri` in this element's scope (shortest, deterministic).
    pub fn prefix_for(&self, uri: &str) -> Option<String> {
        let mut found: Vec<&String> = self.scope.iter().filter(|(_, u)| u.as_str() == uri).map(|(p, _)| p).collect();
        found.sort();
        found.first().map(|p| (*p).clone())
    }
}

#[derive(Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub roots: Vec<usize>,
}

impl Tree {
    pub fn parse(src: &str) -> Result<Tree, String> {
        let mut reader = Reader::from_str(src);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = true;
        let mut tree = Tree::default();
        let mut stack: Vec<usize> = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let event =
                reader.read_event().map_err(|e| format!("invalid XMP: {e} at byte {}", reader.error_position()))?;
            let after = reader.buffer_position() as usize;
            match event {
                Event::Start(_) | Event::Empty(_) => {
                    let empty = matches!(event, Event::Empty(_));
                    let qname: String = src[before + 1..after]
                        .chars()
                        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
                        .collect();
                    let mut attrs = scan_attrs(src, before + 1 + qname.len(), after)?;
                    let mut scope = stack.last().map(|&p| tree.nodes[p].scope.clone()).unwrap_or_default();
                    scope.entry("xml".into()).or_insert_with(|| NS_XML.into());
                    for a in &attrs {
                        if a.prefix == "xmlns" {
                            scope.insert(a.local.clone(), a.value.clone());
                        } else if a.prefix.is_empty() && a.local == "xmlns" {
                            scope.insert(String::new(), a.value.clone());
                        }
                    }
                    for a in &mut attrs {
                        if !a.prefix.is_empty() && a.prefix != "xmlns" {
                            a.uri = scope.get(&a.prefix).cloned().unwrap_or_default();
                        }
                    }
                    let (prefix, local) = split_qname(&qname);
                    let uri = scope.get(&prefix).cloned().unwrap_or_default();
                    let parent = stack.last().copied();
                    let idx = tree.nodes.len();
                    tree.nodes.push(Node {
                        prefix,
                        local,
                        uri,
                        start: before,
                        open_end: after,
                        close_start: after,
                        end: after,
                        attrs,
                        children: Vec::new(),
                        parent,
                        text: String::new(),
                        scope,
                    });
                    match parent {
                        Some(p) => tree.nodes[p].children.push(idx),
                        None => tree.roots.push(idx),
                    }
                    if !empty {
                        stack.push(idx);
                    }
                }
                Event::End(_) => {
                    let idx = stack.pop().ok_or("invalid XMP: unbalanced end tag")?;
                    tree.nodes[idx].close_start = before;
                    tree.nodes[idx].end = after;
                }
                Event::Text(_) | Event::GeneralRef(_) => {
                    if let Some(&top) = stack.last() {
                        let raw = &src[before..after];
                        let text = unescape(raw).map(|c| c.into_owned()).unwrap_or_else(|_| raw.to_owned());
                        tree.nodes[top].text.push_str(&text);
                    }
                }
                Event::CData(_) => {
                    if let Some(&top) = stack.last() {
                        let raw = &src[before..after];
                        let inner = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
                        tree.nodes[top].text.push_str(inner);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if !stack.is_empty() {
            return Err("invalid XMP: unclosed element".into());
        }
        Ok(tree)
    }

    pub fn node(&self, i: usize) -> &Node {
        &self.nodes[i]
    }

    pub fn children<'a>(&'a self, i: usize) -> impl Iterator<Item = (usize, &'a Node)> + 'a {
        self.nodes[i].children.iter().map(move |&c| (c, &self.nodes[c]))
    }

    pub fn child(&self, i: usize, uri: &str, local: &str) -> Option<usize> {
        self.children(i).find(|(_, n)| n.is(uri, local)).map(|(c, _)| c)
    }

    /// Top-level `rdf:Description`s (children of `rdf:RDF`).
    pub fn top_descriptions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if n.is(NS_RDF, "Description") {
                if let Some(p) = n.parent {
                    if self.nodes[p].is(NS_RDF, "RDF") {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    /// Items (`rdf:li`) of the container (`rdf:Seq`/`Bag`/`Alt`) inside property element `prop`.
    pub fn items(&self, prop: usize) -> Vec<usize> {
        let container = self
            .children(prop)
            .find(|(_, n)| n.uri == NS_RDF && matches!(n.local.as_str(), "Seq" | "Bag" | "Alt"))
            .map(|(c, _)| c);
        match container {
            Some(c) => self.children(c).filter(|(_, n)| n.is(NS_RDF, "li")).map(|(i, _)| i).collect(),
            None => Vec::new(),
        }
    }

    /// The nodes carrying a struct value's fields: the element itself (property
    /// attributes / `rdf:parseType="Resource"`) and any `rdf:Description` child.
    pub fn struct_holders(&self, i: usize) -> Vec<usize> {
        let mut out = vec![i];
        out.extend(self.children(i).filter(|(_, n)| n.is(NS_RDF, "Description")).map(|(c, _)| c));
        out
    }

    /// Field `uri:local` of the struct at `i` (attribute or simple element value).
    pub fn field(&self, i: usize, uri: &str, local: &str) -> Option<String> {
        for h in self.struct_holders(i) {
            if let Some(a) = self.nodes[h].attr(uri, local) {
                return Some(a.value.clone());
            }
            if let Some(c) = self.child(h, uri, local) {
                let n = &self.nodes[c];
                if n.children.is_empty() {
                    return Some(n.text.clone());
                }
            }
        }
        None
    }

    /// Child element `uri:local` of the struct at `i`.
    pub fn field_elem(&self, i: usize, uri: &str, local: &str) -> Option<usize> {
        self.struct_holders(i).into_iter().find_map(|h| self.child(h, uri, local))
    }
}

/// Start of the whitespace run that precedes `pos` (back to the previous non-whitespace).
pub fn line_ws_start(src: &str, pos: usize) -> usize {
    let bytes = src.as_bytes();
    let mut i = pos;
    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i
}

/// Indentation (spaces/tabs) of the line containing `pos`, up to `pos`.
pub fn indent_at(src: &str, pos: usize) -> String {
    let line_start = src[..pos].rfind('\n').map_or(0, |i| i + 1);
    src[line_start..pos].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

fn split_qname(q: &str) -> (String, String) {
    match q.split_once(':') {
        Some((p, l)) => (p.to_owned(), l.to_owned()),
        None => (String::new(), q.to_owned()),
    }
}

/// Attributes of the start tag `src[from..tag_end]` (`from` = just after the element name).
fn scan_attrs(src: &str, from: usize, tag_end: usize) -> Result<Vec<Attr>, String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = from;
    loop {
        let ws_start = i;
        while i < tag_end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag_end || b[i] == b'>' || b[i] == b'/' {
            break;
        }
        let name_start = i;
        while i < tag_end && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = &src[name_start..i];
        while i < tag_end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag_end || b[i] != b'=' {
            return Err(format!("invalid XMP: attribute {name} without value"));
        }
        i += 1;
        while i < tag_end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = *b.get(i).ok_or("invalid XMP: truncated attribute")?;
        if quote != b'"' && quote != b'\'' {
            return Err(format!("invalid XMP: unquoted attribute {name}"));
        }
        let vstart = i + 1;
        let vend = src[vstart..tag_end].find(quote as char).map(|k| vstart + k).ok_or("invalid XMP: open attribute")?;
        let raw = &src[vstart..vend];
        let value = unescape(raw).map(|c| c.into_owned()).unwrap_or_else(|_| raw.to_owned());
        let (prefix, local) = split_qname(name);
        out.push(Attr { prefix, local, uri: String::new(), value, ws_start, name_start, end: vend + 1 });
        i = vend + 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spans_attrs_and_namespaces() {
        let src = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" crs:A=\"1 &amp; 2\">\
<crs:S><rdf:Seq><rdf:li>d 0.1 0.2</rdf:li><rdf:li crs:B=\"x\"/></rdf:Seq></crs:S></rdf:Description></rdf:RDF></x:xmpmeta>";
        let t = Tree::parse(src).unwrap();
        let tops = t.top_descriptions();
        assert_eq!(tops.len(), 1);
        let d = t.node(tops[0]);
        let a = d.attr("http://ns.adobe.com/camera-raw-settings/1.0/", "A").unwrap();
        assert_eq!(a.value, "1 & 2");
        assert_eq!(&src[a.name_start..a.end], "crs:A=\"1 &amp; 2\"");
        let s = t.child(tops[0], "http://ns.adobe.com/camera-raw-settings/1.0/", "S").unwrap();
        let items = t.items(s);
        assert_eq!(items.len(), 2);
        assert_eq!(t.node(items[0]).text, "d 0.1 0.2");
        assert_eq!(&src[t.node(items[1]).start..t.node(items[1]).end], "<rdf:li crs:B=\"x\"/>");
        assert_eq!(d.prefix_for("http://ns.adobe.com/camera-raw-settings/1.0/").as_deref(), Some("crs"));
    }
}
