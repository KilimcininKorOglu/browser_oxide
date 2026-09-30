// Trusted Types API (Chrome 83+), shared by window and worker realms. Chrome
// exposes `trustedTypes` on both Window and WorkerGlobalScope.
((globalThis) => {
    // TrustedScript is a native constructor whose instances V8 treats as
    // code-like, so eval(trustedScript) compiles the script as in Chrome.
    // runtime.rs installs it before this bootstrap runs.
    const _nativeTS = globalThis.__ox_trusted_script;
    delete globalThis.__ox_trusted_script;
    if (globalThis.trustedTypes) return;
    const _ttPolicies = new Map();
    const _TrustedHTML = function TrustedHTML(v) { this._v = v; };
    _TrustedHTML.prototype.toString = function() { return this._v; };
    const _tsValues = new WeakMap();
    const _TrustedScript = _nativeTS.ctor;
    const _newTrustedScript = (v) => { const o = _nativeTS.make(); _tsValues.set(o, String(v)); return o; };
    Object.defineProperty(_TrustedScript.prototype, 'toString', { value: function toString() { return _tsValues.get(this); }, writable: true, enumerable: false, configurable: true });
    Object.defineProperty(_TrustedScript.prototype, 'toJSON', { value: function toJSON() { return _tsValues.get(this); }, writable: true, enumerable: false, configurable: true });
    _maskAsNative(_TrustedScript.prototype, 'toString', 'toJSON');
    Object.defineProperty(_TrustedScript.prototype, Symbol.toStringTag, { value: 'TrustedScript', configurable: true });
    const _TrustedScriptURL = function TrustedScriptURL(v) { this._v = v; };
    _TrustedScriptURL.prototype.toString = function() { return this._v; };
    globalThis.trustedTypes = {
        createPolicy(name, rules) {
            const p = {
                name,
                createHTML: (s) => typeof rules.createHTML === 'function' ? new _TrustedHTML(rules.createHTML(s)) : new _TrustedHTML(s),
                createScript: (s) => typeof rules.createScript === 'function' ? _newTrustedScript(rules.createScript(s)) : _newTrustedScript(s),
                createScriptURL: (s) => typeof rules.createScriptURL === 'function' ? new _TrustedScriptURL(rules.createScriptURL(s)) : new _TrustedScriptURL(s),
            };
            _ttPolicies.set(name, p);
            if (name === 'default') globalThis.trustedTypes.defaultPolicy = p;
            return p;
        },
        isHTML(v) { return v instanceof _TrustedHTML; },
        isScript(v) { return _tsValues.has(v); },
        isScriptURL(v) { return v instanceof _TrustedScriptURL; },
        getAttributeType() { return null; },
        getPropertyType() { return null; },
        defaultPolicy: null,
        emptyHTML: new _TrustedHTML(''),
        emptyScript: _newTrustedScript(''),
    };
    globalThis.TrustedHTML = _TrustedHTML;
    globalThis.TrustedScript = _TrustedScript;
    globalThis.TrustedScriptURL = _TrustedScriptURL;
    _maskAsNative(globalThis.trustedTypes, 'createPolicy', 'isHTML', 'isScript', 'isScriptURL', 'getAttributeType', 'getPropertyType');
})(globalThis);
