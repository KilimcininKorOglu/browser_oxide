((globalThis) => {
    const ops = Deno.core.ops;
    const print = (msg) => {
        try { Deno.core.print(msg + "\n"); } catch {}
    };

    // --- Function.prototype.toString bypass patch ---
    // Some scripts call Function.prototype.toString.call(fn)
    // directly, which bypasses any instance-level fn.toString override and
    // returns the raw JS source of polyfilled functions. We patch
    // Function.prototype.toString itself to consult a private Symbol tag
    // we set on masked functions.
    const _nativeTag = Symbol.for('__browser_oxide_native__');
    const _origFnToStr = Function.prototype.toString;

    // Re-entrant guard: prevents infinite recursion when this[_nativeTag] access
    // triggers a Proxy get trap that itself calls Function.prototype.toString.
    let _inPatchedToStr = false;
    // The tag must be the function's OWN property. Read through the
    // prototype chain, `class Foo extends EventTarget {}` inherited
    // EventTarget's tag and printed `function EventTarget() { [native
    // code] }`; Chrome prints the class source.
    const _hasOwn = Function.prototype.call.bind(Object.prototype.hasOwnProperty);
    // Method-shorthand → NO [[Construct]] / no own `.prototype`, exactly
    // like the real native Function.prototype.toString. A plain
    // `function toString(){}` IS constructable, so
    // `class X extends Function.prototype.toString {}` did NOT throw in
    // our engine while real Chrome 147 throws `TypeError`; we match
    // Chrome here.
    const _patchedFnToStr = ({ toString() {
        if (_inPatchedToStr) return _origFnToStr.call(this);
        _inPatchedToStr = true;
        try {
            if (this !== null && this !== undefined) {
                try {
                    const tag = _hasOwn(this, _nativeTag) && this[_nativeTag];
                    if (tag) return `function ${tag}() { [native code] }`;
                } catch (_) {}
            }
            return _origFnToStr.call(this);
        } finally {
            _inPatchedToStr = false;
        }
    } }).toString;
    // Tag the patched toString itself so recursive calls also appear native
    Object.defineProperty(_patchedFnToStr, _nativeTag, { value: 'toString', configurable: true });
    Object.defineProperty(_patchedFnToStr, 'name', { value: 'toString', configurable: true });

    Object.defineProperty(Function.prototype, 'toString', {
        value: _patchedFnToStr,
        writable: true,
        configurable: true,
    });

    // --- Native code masking ---
    // A native function is never an async function: Chrome's fetch or
    // Response.json inherit from Function.prototype and print
    // `[object Function]`. An engine function written `async` inherits from
    // AsyncFunction.prototype, whose toStringTag reads `AsyncFunction`.
    const _AsyncFunctionProto = Object.getPrototypeOf(async function () {});
    const _maskFunction = (fn, name) => {
        if (!fn) return fn;
        try {
            if (Object.getPrototypeOf(fn) === _AsyncFunctionProto) {
                Object.setPrototypeOf(fn, Function.prototype);
            }
            // Native fns have an own configurable `name` (Chrome-correct).
            Object.defineProperty(fn, 'name', { value: name, configurable: true });
            // Symbol tag — read by the patched Function.prototype.toString
            // above. `Symbol.for` (global registry) is DELIBERATE: an
            // iframe realm's patched toString must resolve the tag on
            // PARENT-realm functions (cross-realm robustness — see iframe
            // contentWindow self-loop commits).
            Object.defineProperty(fn, _nativeTag, { value: name, configurable: true });
            // NO own `toString`: it was a self-inflicted leak — an
            // earlier version gave every masked fn an own `toString`, so
            // `getOwnPropertyNames(fn)` included "toString" (Chrome:
            // ['length','name'(,'prototype')] only) and
            // `fn.toString !== Function.prototype.toString` (Chrome: ===,
            // inherited). The own toString was REDUNDANT: the patched
            // Function.prototype.toString already yields
            // `function <tag>() { [native code] }` via the tag, and
            // `fn.toString()` / `Function.prototype.toString.call(fn)`
            // both resolve up-chain to it. Removing it is cross-realm-
            // safe (tag mechanism unchanged) and restores Chrome parity
            // for getOwnPropertyNames / hasOwnProperty('toString') /
            // toString-identity on EVERY masked fn in the engine.
        } catch (e) {}
        // Return fn so callers can use `{ get: _maskFunction(getter, name) }`
        // without the getter silently becoming undefined (property returns undefined).
        return fn;
    };

    // A native method, getter or setter is not a constructor: it has no own
    // `prototype`, and reading its `arguments` or `caller` throws. A
    // `function` expression has both, so one masked as native is told apart
    // (Chrome's `get webdriver` throws on `.arguments`; ours returned null).
    // `_callableLike` swaps such a function for a method-shaped one that
    // forwards to it. A constructor (a class, or a capitalised name) is left
    // alone. One function installed in two places gets one stand-in, so
    // identity between the places holds.
    const _apply = Reflect.apply;
    const _isPlainFunction = (fn) => typeof fn === 'function' && _hasOwn(fn, 'prototype');
    const _isConstructorLike = (fn, name) =>
        /^[A-Z]/.test(String(name)) || /^[A-Z]/.test(String(fn.name)) || /^class\b/.test(_origFnToStr.call(fn));
    const _standIns = new WeakMap();
    const _callableLike = (fn, kind) => {
        let w = _standIns.get(fn);
        if (w) return w;
        if (kind === 'get') {
            w = Object.getOwnPropertyDescriptor({ get x() { return _apply(fn, this, []); } }, 'x').get;
        } else if (kind === 'set') {
            w = Object.getOwnPropertyDescriptor({ set x(/** @type {any} */ v) { _apply(fn, this, [v]); } }, 'x').set;
        } else {
            w = ({ x(...args) { return _apply(fn, this, args); } }).x;
            Object.defineProperty(w, 'length', { value: fn.length, configurable: true });
        }
        _standIns.set(fn, w);
        return w;
    };
    const _nativeShaped = (desc, name) => {
        let changed = false;
        for (const kind of ['get', 'set']) {
            if (_isPlainFunction(desc[kind])) { desc[kind] = _callableLike(desc[kind], kind); changed = true; }
        }
        if (_isPlainFunction(desc.value) && !_isConstructorLike(desc.value, name)) {
            desc.value = _callableLike(desc.value, 'value');
            changed = true;
        }
        return changed;
    };

    const _maskAsNative = (obj, ...names) => {
        for (const name of names) {
            try {
                // Find where the property actually lives (own or prototype)
                let target = obj;
                let desc = Object.getOwnPropertyDescriptor(target, name);
                while (!desc && target && target !== Object.prototype) {
                    target = Object.getPrototypeOf(target);
                    if (target) desc = Object.getOwnPropertyDescriptor(target, name);
                }

                if (desc) {
                    if (desc.configurable && _nativeShaped(desc, name)) Object.defineProperty(target, name, desc);
                    if (desc.get) _maskFunction(desc.get, `get ${name}`);
                    if (desc.set) _maskFunction(desc.set, `set ${name}`);
                    if (typeof desc.value === 'function') _maskFunction(desc.value, name);
                } else {
                    // Fallback for direct prototype access
                    const val = obj[name];
                    if (typeof val === 'function') _maskFunction(val, name);
                }
            } catch (e) {}
        }
    };

    // Expose helpers globally for other bootstraps
    Object.defineProperty(globalThis, '_nativeTag', { value: _nativeTag, enumerable: false, configurable: true });
    Object.defineProperty(globalThis, '_maskFunction', { value: _maskFunction, enumerable: false, configurable: true });
    Object.defineProperty(globalThis, '_maskAsNative', { value: _maskAsNative, enumerable: false, configurable: true });

    // Expose seeded random under a Symbol-keyed
    // slot that survives cleanup_bootstrap's string-keyed `internals`
    // purge. humanize.js (injected per-navigation, AFTER cleanup) reads
    // this via `globalThis[Symbol.for('__browser_oxide_behavior_rand__')]`
    // and uses it instead of Math.random(), so synthetic mouse/scroll/
    // key event streams are deterministic per page lifetime (two-level
    // seed pattern). Falls back to undefined if the op is unavailable —
    // humanize.js degrades to Math.random().
    try {
        const _randOp = Deno && Deno.core && Deno.core.ops
            && Deno.core.ops.op_behavior_random;
        if (typeof _randOp === 'function') {
            const _sym = Symbol.for('__browser_oxide_behavior_rand__');
            Object.defineProperty(globalThis, _sym, {
                value: function () {
                    try { return _randOp(); } catch (_e) { return Math.random(); }
                },
                writable: false, configurable: true, enumerable: false,
            });
        }
    } catch (_e) {}

    // Expose CMU+Buffalo keystroke-schedule
    // generator under a Symbol-keyed slot. humanize.js calls it on
    // input focus to synthesize plausible per-char timings (LogNormal
    // dwell + bigram-modulated flight). The Rust generator existed at
    // crates/stealth/src/behavior.rs but had no JS consumer; this is
    // the wiring.
    try {
        const _ksOp = Deno && Deno.core && Deno.core.ops
            && Deno.core.ops.op_human_keystroke_schedule;
        if (typeof _ksOp === 'function') {
            const _sym = Symbol.for('__browser_oxide_keystroke_schedule__');
            Object.defineProperty(globalThis, _sym, {
                value: function (text, wpm) {
                    try { return _ksOp(text || '', (wpm | 0) || 0); }
                    catch (_e) { return []; }
                },
                writable: false, configurable: true, enumerable: false,
            });
        }
    } catch (_e) {}

    // `eval.toString().length === 33` for Chromium is a known invariant.
    // V8 natively produces "function eval() { [native code] }" (33 chars), so this
    // is usually a no-op. We tag `eval` defensively so any V8 build drift is
    // self-corrected to Chrome's canonical shape.
    try { _maskFunction(eval, 'eval'); } catch (_) {}

    // Native-mask every console method. console_bootstrap.js is
    // concatenated BEFORE this file in the V8 snapshot (snapshot.rs),
    // so it could not call _maskAsNative itself (undefined then) —
    // `globalThis.console` already exists here, and _maskAsNative is
    // now defined, so this is the correct place. Some scripts dump
    // `console.<method>.toString()` for all ~19 methods; without masking,
    // ours would leak `log(...args) { core.ops.op_console_log(...) }`,
    // which differs from real Chrome. Real Chrome returns
    // `function log() { [native code] }` for every console method.
    try {
        if (globalThis.console) {
            _maskAsNative(
                globalThis.console,
                'log', 'warn', 'error', 'info', 'debug', 'dir', 'dirxml',
                'trace', 'group', 'groupCollapsed', 'groupEnd', 'clear',
                'count', 'countReset', 'assert', 'table', 'time',
                'timeLog', 'timeEnd',
            );
        }
    } catch (_) {}

})(globalThis);
