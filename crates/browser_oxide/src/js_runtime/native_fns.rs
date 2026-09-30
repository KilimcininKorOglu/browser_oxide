//! Genuine-native host functions built from raw `v8::FunctionTemplate`
//! (the same primitive deno_core uses for every `#[op2]`, see
//! deno_core's `runtime/bindings.rs`).
//!
//! WHY: V8 builds the `class X extends <fn> {}` TypeError message,
//! `Object::NoSideEffectsToString`, error stacks and `eval`-stringify
//! from a function's internal `[[SourceText]]` via `JSFunction::ToString`,
//! which emits `function NAME() { [native code] }` ONLY when
//! `!SharedFunctionInfo::IsUserJavaScript()` — true exactly for API
//! functions (FunctionTemplate, `script()==undefined`). A JS-level
//! `Function.prototype.toString` patch (stealth_bootstrap.js) CANNOT
//! intercept these internal stringifiers, so our JS shims would leak
//! their source (e.g. `class K extends Function.prototype.toString{}`
//! leaks `_patchedFnToStr` source) — differs from real Chrome, where
//! these are native functions.
//!
//! This installs `Function.prototype.toString` as a genuine API
//! function. Behaviour preserved: masked host fns (carrying the
//! `Symbol.for('__browser_oxide_native__')` tag set by stealth_bootstrap.js)
//! stringify as `function <tag>() { [native code] }`; everything else
//! delegates to the GENUINE original `Function.prototype.toString`
//! (captured BEFORE any bootstrap ran) so real JS user functions still
//! return their source and real natives return `[native code]` — exactly
//! as V8 would. Because the installed function is itself an API
//! function it is non-constructable, `.prototype`-less, and source-less:
//! the class-extends / NoSideEffectsToString leak is structurally gone.

use deno_core::v8;
use std::collections::HashMap;

const NATIVE_TAG: &str = "__browser_oxide_native__";

/// Per-runtime storage for child iframe realms (genuine v8::Context instances).
///
/// Each entry keeps the child `v8::Context` alive (without a Global it would
/// be GC'd) and caches the child global object to avoid re-creating on
/// successive `contentWindow` reads from JS. Keyed by a monotonically-
/// increasing realm ID assigned by `_nextRealmId` in dom_bootstrap.js.
///
/// `orig_fp_tostring` is the builtin `Function.prototype.toString` captured
/// pre-bootstrap; it is installed into every child realm so cross-realm
/// `toString` calls (`cw.Function.prototype.toString.call(parent.fetch)`)
/// produce `[native code]` — same as the main window.
///
/// `native_tag_sym` is the JS-global-registry symbol `Symbol.for('__browser_oxide_native__')`
/// captured after bootstrap runs. It is the SAME symbol object that stealth_bootstrap.js
/// uses to tag masked host functions. The V8 API registry symbol from
/// `v8::Symbol::for_api` is a DIFFERENT registry and will NOT find these tags.
pub struct IframeRealmStore {
    pub contexts: HashMap<u32, v8::Global<v8::Context>>,
    pub globals: HashMap<u32, v8::Global<v8::Object>>,
    pub orig_fp_tostring: Option<v8::Global<v8::Function>>,
    pub native_tag_sym: Option<v8::Global<v8::Symbol>>,
}

impl Default for IframeRealmStore {
    fn default() -> Self {
        Self::new()
    }
}

impl IframeRealmStore {
    pub fn new() -> Self {
        Self {
            contexts: HashMap::new(),
            globals: HashMap::new(),
            orig_fp_tostring: None,
            native_tag_sym: None,
        }
    }
}

/// Create a GENUINE child realm — a second `v8::Context` in the same
/// isolate, exactly what Chrome does per iframe (the primitive
/// deno_core itself uses, `jsruntime.rs:2048`). The child
/// gets its OWN full set of native builtins for free
/// (`Object`/`Function`/`Array`/`Reflect`/`Symbol`/`Map`/`TypeError`/…),
/// each genuinely `[native code]` and realm-distinct from the parent —
/// i.e. NOT a `Proxy` and NOT parent-aliased, matching real Chrome's
/// per-iframe realm semantics.
///
/// This is the PRIMITIVE only (additive, not yet wired into
/// `_getIframeWindow`) — proves the core mechanism works in our
/// single-isolate deno_core engine before the larger wiring pass.
/// Returns the child context's global object as a Global so callers
/// can hold it as `iframe.contentWindow` and keep the context alive.
pub fn create_child_realm(
    scope: &mut v8::PinScope,
) -> Option<(v8::Global<v8::Context>, v8::Global<v8::Object>)> {
    // A fresh context. Default options: the child gets the full native
    // intrinsic set + its own global. (Real Chrome exposes native
    // builtins/prototype-chains/ctor-names per realm — all native here;
    // DOM host shims are a later delegation step.)
    let ctx = v8::Context::new(scope, v8::ContextOptions::default());
    let global = {
        let cscope = &mut v8::ContextScope::new(scope, ctx);
        let g = ctx.global(cscope);
        v8::Global::new(cscope, g)
    };
    Some((v8::Global::new(scope, ctx), global))
}

/// Capture the genuine builtin `Function.prototype.toString` BEFORE any
/// bootstrap replaces it. Returned Global is passed as the API
/// function's `data` so untagged functions delegate to real V8
/// semantics. Call right after `JsRuntime::new`, before bootstrap.
pub fn capture_original_fp_tostring(scope: &mut v8::PinScope) -> Option<v8::Global<v8::Function>> {
    let ctx = scope.get_current_context();
    let global = ctx.global(scope);
    let fkey = v8::String::new(scope, "Function")?;
    let fctor = global.get(scope, fkey.into())?;
    let fctor = v8::Local::<v8::Object>::try_from(fctor).ok()?;
    let pkey = v8::String::new(scope, "prototype")?;
    let fproto = fctor.get(scope, pkey.into())?;
    let fproto = v8::Local::<v8::Object>::try_from(fproto).ok()?;
    let tskey = v8::String::new(scope, "toString")?;
    let ts = fproto.get(scope, tskey.into())?;
    let ts = v8::Local::<v8::Function>::try_from(ts).ok()?;
    Some(v8::Global::new(scope, ts))
}

/// The masked name of `obj`, read from its OWN tag property only: read
/// through the prototype chain, `class Foo extends EventTarget {}` would
/// print as EventTarget's native.
fn own_native_tag(
    scope: &mut v8::PinScope,
    obj: v8::Local<v8::Object>,
    sym: v8::Local<v8::Symbol>,
) -> Option<String> {
    if !obj.has_own_property(scope, sym.into())? {
        return None;
    }
    let tag = obj.get(scope, sym.into())?;
    tag.is_string().then(|| tag.to_rust_string_lossy(scope))
}

/// Whether a Proxy (possibly wrapping further Proxies) ends at a function.
fn is_callable_proxy(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> bool {
    let mut current = value;
    while let Ok(proxy) = v8::Local::<v8::Proxy>::try_from(current) {
        current = proxy.get_target(scope);
    }
    current.is_function()
}

/// The genuine-native `Function.prototype.toString` callback.
///
/// `args.data()` is an Array `[orig, sym]` where:
///   - index 0: the captured genuine `Function.prototype.toString` (v8::Function)
///   - index 1: the JS-global-registry `Symbol.for('__browser_oxide_native__')` (v8::Symbol)
///
/// Using Array data is necessary because V8 callback data can only hold a
/// single v8::Value. The symbol MUST come from the JS global registry
/// (`Symbol::For` / `v8::Symbol::for_key`), not V8's API registry
/// (`Symbol::ForApi` / `v8::Symbol::for_api`) — those are different tables.
/// Stealth_bootstrap.js tags host functions via `Symbol.for('__browser_oxide_native__')`
/// which writes to the JS registry; looking up via `v8::Symbol::for_api`
/// silently misses all tags.
// `v8::Symbol::for_api` is the API-registry lookup we use here only as a
// documented fallback (see the doc comment above); the primary path is the
// JS-registry symbol passed in via the data Array, which we try first.
fn fp_to_string_cb<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let this: v8::Local<v8::Value> = args.this().into();
    let data = args.data();

    // Extract [orig, sym] from Array data, falling back to direct-function
    // data for contexts where no symbol was available at install time.
    let orig_fn: Option<v8::Local<v8::Function>>;
    let tag_sym: Option<v8::Local<v8::Symbol>>;
    if let Ok(arr) = v8::Local::<v8::Array>::try_from(data) {
        orig_fn = arr
            .get_index(scope, 0)
            .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok());
        tag_sym = arr
            .get_index(scope, 1)
            .and_then(|v| v8::Local::<v8::Symbol>::try_from(v).ok());
    } else {
        orig_fn = v8::Local::<v8::Function>::try_from(data).ok();
        tag_sym = None;
    }

    // Chrome prints every callable Proxy as an anonymous native, whatever
    // it wraps, and runs none of its traps. A tag lookup through the Proxy
    // would hand the page's `get` trap our tag symbol.
    if this.is_proxy() {
        if is_callable_proxy(scope, this) {
            if let Some(out) = v8::String::new(scope, "function () { [native code] }") {
                rv.set(out.into());
            }
            return;
        }
    } else if let Ok(this_obj) = v8::Local::<v8::Object>::try_from(this) {
        // Masked host fn: stealth_bootstrap.js sets
        // `fn[Symbol.for('__browser_oxide_native__')] = name`. The primary
        // path uses the JS-registry symbol from the data Array; the V8 API
        // registry symbol is a documented fallback for no-sym contexts.
        let sym = tag_sym.or_else(|| {
            v8::String::new(scope, NATIVE_TAG).map(|key| v8::Symbol::for_api(scope, key))
        });
        if let Some(tag) = sym.and_then(|sym| own_native_tag(scope, this_obj, sym)) {
            let s = format!("function {tag}() {{ [native code] }}");
            if let Some(out) = v8::String::new(scope, &s) {
                rv.set(out.into());
                return;
            }
        }
    }

    // Delegate to the GENUINE original Function.prototype.toString
    // (captured pre-bootstrap, stored at index 0 of the data Array).
    if let Some(orig) = orig_fn {
        if let Some(res) = orig.call(scope, this, &[]) {
            rv.set(res);
        }
        // If orig.call returns None the exception is already set on the
        // scope (e.g. non-callable `this` → TypeError). Just return.
        return;
    }
    // Last-resort fallback (should be unreachable): anonymous native.
    if let Some(out) = v8::String::new(scope, "function () { [native code] }") {
        rv.set(out.into());
    }
}

/// Install the genuine-native `Function.prototype.toString`, replacing
/// the JS-level patch. `original` must be the builtin captured pre-
/// bootstrap (see `capture_original_fp_tostring`). `native_tag_sym` must
/// be `Symbol.for('__browser_oxide_native__')` captured from the JS environment
/// AFTER bootstrap runs — it is the JS-global-registry symbol that
/// stealth_bootstrap.js uses to tag host functions. Pass `None` only when
/// no bootstrap has run (e.g. child realms before symbol is captured).
/// Run AFTER bootstrap + cleanup, before site/init scripts.
pub fn install_native_fp_tostring(
    scope: &mut v8::PinScope,
    original: &v8::Global<v8::Function>,
    native_tag_sym: Option<&v8::Global<v8::Symbol>>,
) -> bool {
    let orig_local = v8::Local::new(scope, original);

    // Pack [orig, sym] into an Array so the callback can access both.
    // A single FunctionTemplate data slot can hold only one v8::Value,
    // so we use an Array to carry the pair.
    let data_val: v8::Local<v8::Value> = if let Some(sym_g) = native_tag_sym {
        let sym_local = v8::Local::new(scope, sym_g);
        let arr = v8::Array::new(scope, 2);
        // v8::Object::set with integer key — guaranteed stable in all rusty_v8 versions.
        let i0 = v8::Integer::new(scope, 0);
        let i1 = v8::Integer::new(scope, 1);
        arr.set(scope, i0.into(), orig_local.into());
        arr.set(scope, i1.into(), sym_local.into());
        arr.into()
    } else {
        // No symbol yet (child realm created before post-bootstrap capture).
        // Fall back to direct function data; callback will use for_api.
        orig_local.into()
    };

    let tmpl = v8::FunctionTemplate::builder(fp_to_string_cb)
        .length(0)
        .constructor_behavior(v8::ConstructorBehavior::Throw)
        .side_effect_type(v8::SideEffectType::HasNoSideEffect)
        .data(data_val)
        .build(scope);
    if let Some(name) = v8::String::new(scope, "toString") {
        tmpl.set_class_name(name);
    }
    let func = match tmpl.get_function(scope) {
        Some(f) => f,
        None => return false,
    };
    if let Some(name) = v8::String::new(scope, "toString") {
        func.set_name(name);
    }

    // Install on Function.prototype with Chrome's attribute shape:
    // { value, writable:true, enumerable:false, configurable:true }.
    let ctx = scope.get_current_context();
    let global = ctx.global(scope);
    let Some(fkey) = v8::String::new(scope, "Function") else {
        return false;
    };
    let Some(fctor) = global.get(scope, fkey.into()) else {
        return false;
    };
    let Ok(fctor) = v8::Local::<v8::Object>::try_from(fctor) else {
        return false;
    };
    let Some(pkey) = v8::String::new(scope, "prototype") else {
        return false;
    };
    let Some(fproto) = fctor.get(scope, pkey.into()) else {
        return false;
    };
    let Ok(fproto) = v8::Local::<v8::Object>::try_from(fproto) else {
        return false;
    };
    let Some(tskey) = v8::String::new(scope, "toString") else {
        return false;
    };
    // define_own_property with DONT_ENUM (writable+configurable default).
    fproto.define_own_property(
        scope,
        tskey.into(),
        func.into(),
        v8::PropertyAttribute::DONT_ENUM,
    );
    true
}

/// The script name of every engine bootstrap script. Its frames are dropped
/// from page-visible stacks, as Chrome's stacks never show browser internals.
pub const INTERNAL_SCRIPT_NAME: &str = "<internal>";

/// Engine-internal scripts whose frames never reach a page-visible stack:
/// deno_core's `ext:` / `deno:` modules and bootstrap scripts named `<...>`
/// such as [`INTERNAL_SCRIPT_NAME`]. V8's own `<anonymous>` stays.
fn is_internal_script(name: &str) -> bool {
    name.starts_with("ext:")
        || name.starts_with("deno:")
        || name.contains("core/")
        || (name.starts_with('<') && name.ends_with('>') && name != "<anonymous>")
}

fn call_method<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    obj: v8::Local<'s, v8::Object>,
    name: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let key = v8::String::new(scope, name)?;
    let method = v8::Local::<v8::Function>::try_from(obj.get(scope, key.into())?).ok()?;
    method.call(scope, obj.into(), &[])
}

fn page_frames<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    callsites: v8::Local<'s, v8::Array>,
) -> Vec<v8::Local<'s, v8::Object>> {
    let mut frames = Vec::new();
    for i in 0..callsites.length() {
        let Some(site) = callsites
            .get_index(scope, i)
            .and_then(|v| v.to_object(scope))
        else {
            continue;
        };
        let file = call_method(scope, site, "getFileName")
            .filter(|v| v.is_string())
            .map(|v| v.to_rust_string_lossy(scope))
            .unwrap_or_default();
        if !is_internal_script(&file) {
            frames.push(site);
        }
    }
    frames
}

/// `ErrorUtils::ToString`: the `name: message` line V8 heads a stack with.
fn error_header<'s>(scope: &mut v8::PinScope<'s, '_>, error: v8::Local<'s, v8::Value>) -> String {
    let Some(obj) = error.to_object(scope) else {
        return error.to_rust_string_lossy(scope);
    };
    let read = |key: &str, default: &str| -> String {
        v8::String::new(scope, key)
            .and_then(|k| obj.get(scope, k.into()))
            .filter(|v| !v.is_undefined())
            .map(|v| v.to_rust_string_lossy(scope))
            .unwrap_or_else(|| default.to_string())
    };
    let name = read("name", "Error");
    let message = read("message", "");
    match (name.is_empty(), message.is_empty()) {
        (true, _) => message,
        (false, true) => name,
        (false, false) => format!("{name}: {message}"),
    }
}

/// The page's own `Error.prepareStackTrace`, with `Error` as its receiver.
fn page_prepare_stack_trace<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Option<(v8::Local<'s, v8::Object>, v8::Local<'s, v8::Function>)> {
    let global = scope.get_current_context().global(scope);
    let error_key = v8::String::new(scope, "Error")?;
    let error_ctor = global.get(scope, error_key.into())?.to_object(scope)?;
    let key = v8::String::new(scope, "prepareStackTrace")?;
    let prepare = v8::Local::<v8::Function>::try_from(error_ctor.get(scope, key.into())?).ok()?;
    Some((error_ctor, prepare))
}

/// Builds `error.stack` the way Chrome does, without a JS-visible
/// `Error.prepareStackTrace` of our own: frames from engine-internal scripts
/// are dropped, a page-installed `Error.prepareStackTrace` receives the rest,
/// and otherwise each frame is V8's own `CallSite.prototype.toString`
/// (`at String.fromCodePoint (<anonymous>)`, `at new K (...)`, eval origins).
pub fn chrome_prepare_stack_trace<'s, 'i>(
    scope: &mut v8::PinScope<'s, 'i>,
    error: v8::Local<'s, v8::Value>,
    callsites: v8::Local<'s, v8::Array>,
) -> v8::Local<'s, v8::Value> {
    let frames = page_frames(scope, callsites);
    if let Some((error_ctor, prepare)) = page_prepare_stack_trace(scope) {
        let sites: Vec<v8::Local<v8::Value>> = frames.iter().map(|f| (*f).into()).collect();
        let sites = v8::Array::new_with_elements(scope, &sites);
        return prepare
            .call(scope, error_ctor.into(), &[error, sites.into()])
            .unwrap_or_else(|| v8::undefined(scope).into());
    }
    let mut stack = error_header(scope, error);
    for frame in frames {
        if let Some(line) = call_method(scope, frame, "toString") {
            stack.push_str("\n    at ");
            stack.push_str(&line.to_rust_string_lossy(scope));
        }
    }
    v8::String::new(scope, &stack)
        .map(Into::into)
        .unwrap_or_else(|| v8::undefined(scope).into())
}

extern "C" {
    // `v8::ObjectTemplate::SetCodeLike()`, which the rusty_v8 bindings do
    // not wrap. A rusty_v8 `&ObjectTemplate` is the handle slot address,
    // the same pointer a C++ `Local<ObjectTemplate>` dereferences to, so it
    // is a valid `this` (rusty_v8's own shims rely on the same identity).
    #[link_name = "_ZN2v814ObjectTemplate11SetCodeLikeEv"]
    fn v8_object_template_set_code_like(this: *const v8::ObjectTemplate);
}

thread_local! {
    /// Set only while the `make` factory constructs an instance, so a
    /// script-level `new TrustedScript()` still throws like Chrome.
    static TRUSTED_SCRIPT_CONSTRUCTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn trusted_script_ctor_cb<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue,
) {
    if TRUSTED_SCRIPT_CONSTRUCTING.with(|c| c.get()) {
        return;
    }
    if let Some(msg) = v8::String::new(scope, "Illegal constructor") {
        let err = v8::Exception::type_error(scope, msg);
        scope.throw_exception(err);
    }
}

fn trusted_script_make_cb<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue,
) {
    let Ok(ctor) = v8::Local::<v8::Function>::try_from(args.data()) else {
        return;
    };
    TRUSTED_SCRIPT_CONSTRUCTING.with(|c| c.set(true));
    let obj = ctor.new_instance(scope, &[]);
    TRUSTED_SCRIPT_CONSTRUCTING.with(|c| c.set(false));
    if let Some(obj) = obj {
        rv.set(obj.into());
    }
}

extern "C" {
    // `v8::Isolate::SetModifyCodeGenerationFromStringsCallback()` and
    // `v8::Context::SetErrorMessageForCodeGenerationFromStrings()`, which the
    // rusty_v8 bindings do not wrap. A `Local<T>` is passed as the address of
    // its handle slot, the same identity the SetCodeLike shim relies on.
    #[link_name = "_ZN2v87Isolate42SetModifyCodeGenerationFromStringsCallbackEPFNS_37ModifyCodeGenerationFromStringsResultENS_5LocalINS_7ContextEEENS2_INS_5ValueEEEbE"]
    fn v8_isolate_set_modify_codegen_callback(
        this: *mut std::ffi::c_void,
        callback: ModifyCodegenCallback,
    );
    #[link_name = "_ZN2v87Context43SetErrorMessageForCodeGenerationFromStringsENS_5LocalINS_6StringEEE"]
    fn v8_context_set_codegen_error_message(this: *const v8::Context, message: *const v8::String);
    #[link_name = "_ZNK2v86Object10IsCodeLikeEPNS_7IsolateE"]
    fn v8_object_is_code_like(this: *const v8::Object, isolate: *mut std::ffi::c_void) -> bool;
}

/// The C++ isolate behind a rusty_v8 `Isolate`, which is
/// `#[repr(transparent)]` over exactly that pointer.
fn raw_isolate(isolate: &v8::Isolate) -> *mut std::ffi::c_void {
    // SAFETY: see above; the read copies the wrapped pointer.
    unsafe { *(isolate as *const v8::Isolate as *const *mut std::ffi::c_void) }
}

/// `v8::ModifyCodeGenerationFromStringsResult`: a bool and a
/// `MaybeLocal<String>`, which is one handle-slot pointer or null.
#[repr(C)]
struct ModifyCodegenResult {
    codegen_allowed: bool,
    modified_source: *const v8::String,
}

type ModifyCodegenCallback = for<'s> extern "C" fn(
    v8::Local<'s, v8::Context>,
    v8::Local<'s, v8::Value>,
    bool,
) -> ModifyCodegenResult;

/// The EvalError message Chrome gives when Trusted Types refuse a string.
const TRUSTED_TYPES_EVAL_MESSAGE: &str =
    "Evaluating a string as JavaScript violates this document's Trusted Type assignment requirements.";

/// The realm's default-policy check, registered by trusted_types_bootstrap.js:
/// `(source) => boolean`, true when the default policy passes `source`
/// through unchanged.
struct TrustedScriptCheck(v8::Global<v8::Function>);

fn trusted_script_check_cb<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue,
) {
    if let Ok(hook) = v8::Local::<v8::Function>::try_from(args.get(0)) {
        let hook = v8::Global::new(scope, hook);
        scope
            .get_current_context()
            .set_slot(std::rc::Rc::new(TrustedScriptCheck(hook)));
    }
}

fn default_policy_accepts<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    context: v8::Local<'s, v8::Context>,
    source: v8::Local<'s, v8::String>,
) -> bool {
    let Some(check) = context.get_slot::<TrustedScriptCheck>() else {
        return false;
    };
    let hook = v8::Local::new(scope, &check.0);
    // Chrome refuses the string when the default policy throws, so the
    // exception stops here and V8 raises the EvalError instead.
    v8::tc_scope!(let tc, scope);
    let receiver = v8::undefined(tc).into();
    hook.call(tc, receiver, &[source.into()])
        .is_some_and(|v| v.is_true())
}

/// Chrome's code-generation check under `require-trusted-types-for 'script'`:
/// a TrustedScript handed to `eval` compiles as its text, and a string
/// (what `Function` always passes) compiles only when the default policy
/// returns it unchanged. V8's `eval` does not report a code-like argument
/// as such, so the object is tested here, as Chrome tests for a
/// TrustedScript.
extern "C" fn trusted_types_codegen_cb<'s>(
    context: v8::Local<'s, v8::Context>,
    source: v8::Local<'s, v8::Value>,
    _is_code_like: bool,
) -> ModifyCodegenResult {
    let blocked = ModifyCodegenResult {
        codegen_allowed: false,
        modified_source: std::ptr::null(),
    };
    // No handle scope of its own: the returned source must outlive this call.
    v8::callback_scope!(unsafe scope, context);
    if let Ok(text) = v8::Local::<v8::String>::try_from(source) {
        if default_policy_accepts(scope, context, text) {
            return ModifyCodegenResult {
                codegen_allowed: true,
                modified_source: &*text,
            };
        }
        return blocked;
    }
    let code_like = v8::Local::<v8::Object>::try_from(source).is_ok_and(|obj| {
        // SAFETY: `obj` is a live handle and the isolate is the one running.
        unsafe { v8_object_is_code_like(&*obj, raw_isolate(scope)) }
    });
    if !code_like {
        // Neither a string nor a TrustedScript: eval returns it unchanged.
        return ModifyCodegenResult {
            codegen_allowed: true,
            modified_source: std::ptr::null(),
        };
    }
    v8::tc_scope!(let tc, scope);
    match source.to_string(tc) {
        Some(text) => ModifyCodegenResult {
            codegen_allowed: true,
            modified_source: &*text,
        },
        None => blocked,
    }
}

/// Enforce `require-trusted-types-for 'script'` on `eval` and `Function` in
/// the current context, as Chrome does for a document whose CSP carries it.
pub fn enforce_trusted_types_for_script(scope: &mut v8::PinScope) {
    let context = scope.get_current_context();
    context.set_allow_generation_from_strings(false);
    if let Some(message) = v8::String::new(scope, TRUSTED_TYPES_EVAL_MESSAGE) {
        // SAFETY: both handles are live in `scope`; see the extern block.
        unsafe { v8_context_set_codegen_error_message(&*context, &*message) };
    }
    // SAFETY: the pointer is the running isolate; see `raw_isolate`.
    unsafe { v8_isolate_set_modify_codegen_callback(raw_isolate(scope), trusted_types_codegen_cb) };
}

/// Install the native half of `TrustedScript` as the one-shot global
/// `__ox_trusted_script = { ctor, make, check }`, which
/// trusted_types_bootstrap.js reads and deletes.
///
/// Chrome marks a TrustedScript's instance template code-like, so V8's
/// `eval` and `Function` compile the object's string value instead of
/// returning the object untouched. V8 only treats an object as code-like
/// when its constructor is an API function whose instance template carries
/// that flag, so a JS-level class cannot provide this. Run before bootstrap.
pub fn install_trusted_script_native(scope: &mut v8::PinScope) -> bool {
    let tmpl = v8::FunctionTemplate::builder(trusted_script_ctor_cb)
        .length(0)
        .build(scope);
    let Some(class_name) = v8::String::new(scope, "TrustedScript") else {
        return false;
    };
    tmpl.set_class_name(class_name);
    let instance = tmpl.instance_template(scope);
    // SAFETY: `instance` is a live handle in `scope`; see the extern block.
    unsafe { v8_object_template_set_code_like(&*instance) };
    let Some(ctor) = tmpl.get_function(scope) else {
        return false;
    };
    ctor.set_name(class_name);
    let make = v8::FunctionTemplate::builder(trusted_script_make_cb)
        .data(ctor.into())
        .constructor_behavior(v8::ConstructorBehavior::Throw)
        .build(scope);
    let Some(make) = make.get_function(scope) else {
        return false;
    };
    let holder = v8::Object::new(scope);
    let (Some(k_ctor), Some(k_make), Some(k_global)) = (
        v8::String::new(scope, "ctor"),
        v8::String::new(scope, "make"),
        v8::String::new(scope, "__ox_trusted_script"),
    ) else {
        return false;
    };
    let check = v8::FunctionTemplate::builder(trusted_script_check_cb)
        .constructor_behavior(v8::ConstructorBehavior::Throw)
        .build(scope);
    let (Some(check), Some(k_check)) = (check.get_function(scope), v8::String::new(scope, "check"))
    else {
        return false;
    };
    holder.set(scope, k_ctor.into(), ctor.into());
    holder.set(scope, k_make.into(), make.into());
    holder.set(scope, k_check.into(), check.into());
    let global = scope.get_current_context().global(scope);
    global
        .set(scope, k_global.into(), holder.into())
        .unwrap_or(false)
}

fn run_classic_script_cb<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _rv: v8::ReturnValue,
) {
    let (Some(source), Some(name)) = (args.get(0).to_string(scope), args.get(1).to_string(scope))
    else {
        return;
    };
    let origin = v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    // A compile or run failure leaves V8's exception pending, so it reaches
    // the JS caller as a throw.
    if let Some(script) = v8::Script::compile(scope, source, Some(&origin)) {
        script.run(scope);
    }
}

/// Install `__ox_run_classic_script(source, name)` as a one-shot global,
/// which dom_bootstrap.js and worker_bootstrap.js read and delete.
///
/// Chrome compiles a script-inserted `<script>` and an `importScripts` file
/// as a script of its own, named by its URL (an inline one by the empty
/// string). Running them through `eval` instead gives every frame an
/// `eval at ...` origin that points into the bootstrap. Run before bootstrap.
pub fn install_classic_script_runner(scope: &mut v8::PinScope) -> bool {
    let tmpl = v8::FunctionTemplate::builder(run_classic_script_cb)
        .length(2)
        .constructor_behavior(v8::ConstructorBehavior::Throw)
        .build(scope);
    let (Some(func), Some(key)) = (
        tmpl.get_function(scope),
        v8::String::new(scope, "__ox_run_classic_script"),
    ) else {
        return false;
    };
    let global = scope.get_current_context().global(scope);
    global.set(scope, key.into(), func.into()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deno_core::{JsRuntime, RuntimeOptions};

    /// Verifies that `proxy.get_prototype()` returns the inner global (not
    /// Window.prototype), and that `create_data_property` on the inner global
    /// creates properties visible from INSIDE the child realm via script eval.
    /// Also tests that calling `set_prototype()` on the proxy (as done by
    /// `op_create_child_realm`) doesn't change what `get_prototype()` returns.
    #[test]
    fn verify_inner_global_property_visibility() {
        let mut rt = JsRuntime::new(RuntimeOptions::default());
        let main_ctx = rt.main_context();
        v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);

        let child_ctx = v8::Context::new(scope, v8::ContextOptions::default());
        {
            let cs = &mut v8::ContextScope::new(scope, child_ctx);

            // Simulate what op_create_child_realm does: set a Window prototype
            let window_proto_src = v8::String::new(cs, "(function Window(){}).prototype").unwrap();
            let window_proto_script = v8::Script::compile(cs, window_proto_src, None).unwrap();
            let window_proto_val = window_proto_script.run(cs).unwrap();

            let proxy = child_ctx.global(cs);
            proxy.set_prototype(cs, window_proto_val); // mimic op_create_child_realm

            // Now get inner global via get_prototype (must still be inner global, not window_proto)
            let proto_after = proxy
                .get_prototype(cs)
                .expect("proxy must have a prototype");
            let inner = v8::Local::<v8::Object>::try_from(proto_after)
                .expect("prototype after set_prototype must still be an Object (inner global)");

            // The inner global must NOT be the window_proto we set
            // (set_prototype sets inner_global's [[Prototype]], not the proxy's)
            let inner_hash = inner.get_identity_hash();
            let proxy_hash = proxy.get_identity_hash();
            assert_ne!(
                inner_hash, proxy_hash,
                "inner global must differ from proxy"
            );

            let key = v8::String::new(cs, "__testProp__").unwrap();
            let val = v8::Integer::new(cs, 42);
            inner.create_data_property(cs, key.into(), val.into());

            // Property must be readable from inside the realm via eval
            let src = v8::String::new(cs, "typeof __testProp__ + ':' + __testProp__").unwrap();
            let script = v8::Script::compile(cs, src, None).unwrap();
            let res = script.run(cs).unwrap().to_rust_string_lossy(cs);
            assert_eq!(res, "number:42",
                "inner-global property must be visible inside realm after set_prototype; got: {res}");
        }
    }

    /// Proves the child-realm PRIMITIVE: a raw `v8::Context::new` child
    /// in the same isolate has its OWN genuine native intrinsics
    /// (`[native code]`, correct ctor names) and a global object
    /// distinct from the parent — i.e. exactly what real Chrome exposes
    /// for `iframe.contentWindow` (real realm, NOT a Proxy, NOT
    /// parent-aliased). Foundation for the _getIframeWindow wiring.
    #[test]
    fn child_realm_has_genuine_native_intrinsics() {
        let mut rt = JsRuntime::new(RuntimeOptions::default());
        let main_ctx = rt.main_context();
        v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);

        // Parent realm Object identity (for distinctness check).
        let parent_obj_hash = {
            let g = scope.get_current_context().global(scope);
            let k = v8::String::new(scope, "Object").unwrap();
            let o = g.get(scope, k.into()).unwrap();
            v8::Local::<v8::Object>::try_from(o)
                .unwrap()
                .get_identity_hash()
        };

        let (ctx_g, _glob_g) = create_child_realm(scope).expect("child realm created");

        let ctx = v8::Local::new(scope, &ctx_g);
        let cs = &mut v8::ContextScope::new(scope, ctx);

        // Run a probe IN the child realm.
        let src = v8::String::new(
            cs,
            "JSON.stringify({\
               objTS: Function.prototype.toString.call(Object),\
               fnName: Function.name,\
               objName: Object.name,\
               arrTS: Array.prototype.slice.toString(),\
               typeofWin: typeof globalThis,\
               isProxyish: (function(){try{return String(globalThis).indexOf('Proxy')>=0}catch(e){return 'err'}})()\
             })",
        )
        .unwrap();
        let script = v8::Script::compile(cs, src, None).unwrap();
        let res = script.run(cs).unwrap();
        let json = res.to_rust_string_lossy(cs);

        // Child intrinsics must be GENUINE natives.
        assert!(
            json.contains("function Object() { [native code] }"),
            "child Object must be a real native, got: {json}"
        );
        assert!(
            json.contains("function slice() { [native code] }"),
            "child Array.prototype.slice must be native, got: {json}"
        );
        assert!(
            json.contains("\"objName\":\"Object\""),
            "child Object.name must be 'Object', got: {json}"
        );

        // Child global Object must be a DISTINCT object from parent's.
        let child_obj_hash = {
            let g = ctx.global(cs);
            let k = v8::String::new(cs, "Object").unwrap();
            let o = g.get(cs, k.into()).unwrap();
            v8::Local::<v8::Object>::try_from(o)
                .unwrap()
                .get_identity_hash()
        };
        assert_ne!(
            parent_obj_hash, child_obj_hash,
            "child realm Object must be realm-distinct from parent \
             (real per-frame realm, not parent-aliased)"
        );
    }

    /// Verifies Fix A (Symbol registry): `install_native_fp_tostring` with the
    /// JS-global-registry Symbol returns `function <tag>() { [native code] }`
    /// for tagged functions, instead of falling through to the wrong registry.
    ///
    /// This is the regression test for the v8::Symbol::for_global vs
    /// Symbol::For bug: `for_global` uses the API registry (Symbol::ForApi),
    /// not the JS global registry (Symbol::For). Tags set via Symbol.for() in
    /// JS are INVISIBLE to for_global lookups.
    #[test]
    fn native_fp_tostring_uses_js_symbol_registry() {
        use deno_core::JsRuntime;

        let mut rt = JsRuntime::new(RuntimeOptions::default());

        // Capture original FP.toString (pre-bootstrap).
        let orig_g = {
            let main_ctx = rt.main_context();
            v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);
            capture_original_fp_tostring(scope).expect("capture original")
        };

        // Simulate bootstrap: set Symbol.for('__browser_oxide_native__') tag on a function.
        rt.execute_script(
            "<test>",
            r#"
                const _nativeTag = Symbol.for('__browser_oxide_native__');
                function myTaggedFn() {}
                Object.defineProperty(myTaggedFn, _nativeTag, { value: 'myTaggedFn', configurable: true });
                globalThis.__testFn = myTaggedFn;
            "#,
        )
        .expect("setup script");

        // Capture Symbol.for('__browser_oxide_native__') from JS environment.
        let native_tag_sym_g: Option<v8::Global<v8::Symbol>> = {
            let main_ctx = rt.main_context();
            v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);
            let src = v8::String::new(scope, "Symbol.for('__browser_oxide_native__')").unwrap();
            let script = v8::Script::compile(scope, src, None).unwrap();
            let val = script.run(scope).unwrap();
            let sym = v8::Local::<v8::Symbol>::try_from(val).ok().unwrap();
            Some(v8::Global::new(scope, sym))
        };

        // Install native FP.toString with the correct symbol.
        {
            let main_ctx = rt.main_context();
            v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);
            install_native_fp_tostring(scope, &orig_g, native_tag_sym_g.as_ref());
        }

        // Test: FP.toString.call(myTaggedFn) should return tagged string.
        let s = rt
            .execute_script(
                "<test>",
                "Function.prototype.toString.call(globalThis.__testFn)",
            )
            .map(|v| {
                let main_ctx = rt.main_context();
                v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);
                let local = v8::Local::new(scope, &v);
                local.to_rust_string_lossy(scope)
            })
            .expect("eval");
        assert_eq!(
            s, "function myTaggedFn() { [native code] }",
            "tagged function should return native-code string; got: {s}"
        );

        // Test: FP.toString.call(Array.prototype.slice) should also work
        // (real native - should return native string via orig delegation).
        let s2 = rt
            .execute_script(
                "<test>",
                "Function.prototype.toString.call(Array.prototype.slice)",
            )
            .map(|v| {
                let main_ctx = rt.main_context();
                v8::scope_with_context!(let scope, rt.v8_isolate(), &main_ctx);
                let local = v8::Local::new(scope, &v);
                local.to_rust_string_lossy(scope)
            })
            .expect("eval2");
        assert!(
            s2.contains("[native code]"),
            "real native should return [native code]; got: {s2}"
        );
    }
}
