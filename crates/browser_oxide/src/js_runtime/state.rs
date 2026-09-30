use crate::dom::node::NodeId;
use crate::dom::Dom;
use crate::layout::{LayoutEngine, Viewport};
use std::collections::HashMap;
use std::sync::Arc;

/// Shared state stored in deno_core's OpState, accessible by all ops.
pub struct DomState {
    pub dom: Dom,
    pub layout_engine: LayoutEngine,
    pub base_url: Option<url::Url>,
    /// Console output capture
    pub console_output: Vec<ConsoleMessage>,
    /// localStorage / sessionStorage (in-memory)
    pub storage: HashMap<String, HashMap<String, String>>,
    /// CSS fetched for the document's `<link rel="stylesheet">` elements.
    pub external_stylesheets: Vec<String>,
    /// Rule lists of the `<style>` sheets that script edited through
    /// CSSOM, keyed by the style element.
    pub sheet_rules: HashMap<NodeId, SheetRules>,
    /// The sheets in effect: every connected `<style>` element in
    /// document order, then the external sheets. Rebuilt by
    /// `refresh_styles` after the DOM changes.
    pub stylesheets: Vec<String>,
    /// Parsed and simplified CSS rules for fast lookup
    pub cached_rules: Vec<CachedRule>,
    /// Set by a DOM mutation; `refresh_styles` then rebuilds the sheets.
    pub styles_dirty: bool,
    /// Counts DOM changes, so a live computed style knows to read again.
    pub style_generation: u32,
    pub stealth_profile: Option<crate::stealth::StealthProfile>,
    /// Active Content Security Policy. Built from the response
    /// `Content-Security-Policy` header(s) plus any
    /// `<meta http-equiv="Content-Security-Policy">` tags found in the
    /// parsed HTML. None means no policy applies (e.g. about:blank,
    /// from_html with no header). The policy applies to ALL fetches —
    /// `<script src>`, `op_fetch`, `op_net_fetch_sync`, iframes — until
    /// the next top-level navigation.
    pub csp_policy: Option<Arc<crate::net::csp::PolicySet>>,
    /// Origin used to resolve `'self'` in CSP source matching. Equals
    /// the document's origin (scheme + host + port of the navigated
    /// URL). None for opaque/about:blank documents — those bypass CSP.
    pub csp_origin: Option<url::Url>,
    /// Resource timings for performance.getEntriesByType('resource')
    pub resource_timings: Vec<crate::net::TimingStats>,
}

#[derive(Debug, Clone)]
pub struct CachedRule {
    pub selector_str: String,
    pub selectors: crate::css_selectors::SelectorList,
    pub declarations: HashMap<String, String>,
}

/// A `<style>` sheet's CSSOM rule list, valid while the element's
/// text stays `source`. Chrome builds a new sheet when the text changes.
#[derive(Debug, Clone)]
pub struct SheetRules {
    pub source: String,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ConsoleMessage {
    pub level: ConsoleLevel,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Log,
    Warn,
    Error,
    Info,
    Debug,
}

impl DomState {
    pub fn new(dom: Dom) -> Self {
        let mut storage = HashMap::new();
        storage.insert("local".to_string(), HashMap::new());
        storage.insert("session".to_string(), HashMap::new());
        Self {
            dom,
            layout_engine: LayoutEngine::new(Viewport::new(1920.0, 1080.0)),
            base_url: None,
            console_output: Vec::new(),
            storage,
            external_stylesheets: Vec::new(),
            sheet_rules: HashMap::new(),
            stylesheets: Vec::new(),
            cached_rules: Vec::new(),
            styles_dirty: true,
            style_generation: 0,
            stealth_profile: None,
            csp_policy: None,
            csp_origin: None,
            resource_timings: Vec::new(),
        }
    }

    /// Record a DOM change: the sheets and the layout are rebuilt on the
    /// next read.
    pub fn invalidate_styles(&mut self) {
        self.styles_dirty = true;
        self.style_generation = self.style_generation.wrapping_add(1);
        self.layout_engine.mark_dirty();
    }

    /// Rebuild the sheets in effect when the DOM changed since the last
    /// call, and hand them to the layout engine when they differ.
    pub fn refresh_styles(&mut self) {
        if !self.styles_dirty {
            return;
        }
        self.styles_dirty = false;
        let nodes = crate::stylesheet_collector::style_elements(&self.dom);
        let dom = &self.dom;
        self.sheet_rules
            .retain(|id, sheet| nodes.contains(id) && dom.text_content(*id) == sheet.source);
        let mut sheets: Vec<String> = nodes.iter().map(|id| self.sheet_text(*id)).collect();
        sheets.extend(self.external_stylesheets.iter().cloned());
        if sheets != self.stylesheets {
            self.stylesheets = sheets;
            self.update_cached_rules();
            self.layout_engine.set_stylesheets(&self.stylesheets);
        }
    }

    /// The CSS a `<style>` element contributes: its CSSOM rules when
    /// script edited them, else its text.
    pub fn sheet_text(&self, id: NodeId) -> String {
        match self.sheet_rules.get(&id) {
            Some(sheet) => sheet.rules.join("\n"),
            None => self.dom.text_content(id),
        }
    }

    pub fn update_cached_rules(&mut self) {
        use crate::js_runtime::utils::tokens_to_string;
        self.cached_rules.clear();
        for css_text in &self.stylesheets {
            let (stylesheet, _errors) = crate::css_parser::parse_stylesheet(css_text);
            for rule in &stylesheet.rules {
                if let crate::css_parser::ast::Rule::Qualified(qr) = rule {
                    let selector_str = tokens_to_string(&qr.prelude);
                    if selector_str.is_empty() {
                        continue;
                    }
                    let mut declarations = HashMap::new();
                    for d in &qr.declarations {
                        declarations.insert(
                            d.name.to_string(),
                            tokens_to_string(&d.value).trim().to_string(),
                        );
                    }
                    let selectors = crate::css_selectors::parse_selector_list(&selector_str)
                        .unwrap_or_default();
                    self.cached_rules.push(CachedRule {
                        selector_str,
                        selectors,
                        declarations,
                    });
                }
            }
        }
    }

    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.base_url = Some(url);
        self
    }
}
