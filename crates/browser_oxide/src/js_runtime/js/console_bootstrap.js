((globalThis) => {
    const core = Deno.core;

    // Error constructors the engine uses to raise its own errors, wrapped so
    // the capture happens with a reserve.
    //
    // V8 stops collecting frames at `Error.stackTraceLimit`, and it does so
    // BEFORE the stack formatter runs. Chrome's engine-thrown errors come from
    // C++ and occupy no frame at all, so a script that shrinks the limit —
    // which is exactly what a challenge does, to hide its own frames — still
    // reads a full complement of ITS frames back. Ours are built in JS and sit
    // between the script and the throw site, so the same limit left a script
    // with nothing at all: measured, limit 3 gave 3 frames in Chrome and 0
    // here. Building with the limit raised, and trimming back afterwards in
    // the stack formatter (native_fns::page_frames), makes the count a script
    // reads the one it would read in Chrome.
    //
    // `new X(...)` still works through these, so the call sites read the same
    // and the result is a real instance of the real constructor.
    {
        const _wrapErrorCtor = (C) => {
            const W = function (...args) {
                let lim;
                try { lim = Error.stackTraceLimit; } catch (_) {}
                const bump = typeof lim === 'number' && lim >= 0 && lim < Infinity;
                if (bump) { try { Error.stackTraceLimit = lim + 16; } catch (_) {} }
                try {
                    return Reflect.construct(C, args);
                } finally {
                    if (bump) { try { Error.stackTraceLimit = lim; } catch (_) {} }
                }
            };
            Object.defineProperty(W, 'name', { value: C.name, configurable: true });
            Object.defineProperty(W, 'length', { value: C.length, configurable: true });
            return W;
        };
        // Resolved on first use: DOMException does not exist yet at this
        // point in the bootstrap, and nothing raises an error before every
        // file has run anyway.
        const _oxT = {};
        for (const _name of ['TypeError', 'RangeError', 'ReferenceError', 'EvalError',
            'SyntaxError', 'URIError', 'AggregateError', 'DOMException']) {
            Object.defineProperty(_oxT, _name, {
                configurable: true,
                get() {
                    const C = globalThis[_name];
                    if (typeof C !== 'function') return C;
                    const W = _wrapErrorCtor(C);
                    Object.defineProperty(_oxT, _name, {
                        value: W, writable: true, enumerable: true, configurable: true,
                    });
                    return W;
                },
            });
        }
        Object.defineProperty(globalThis, '__oxT', {
            value: _oxT, writable: true, enumerable: false, configurable: true,
        });
    }

    // Read a property WITHOUT invoking a page-defined accessor. Real
    // V8 `Error.stack`/`message` are own *data* properties and
    // `name`/`constructor` are data properties on the prototype chain;
    // a planted `Object.defineProperty(e,'stack',{get(){…}})` is an own
    // *accessor*. console.* eagerly stringifying via `arg.stack` would
    // invoke that getter — exactly the svebaa CDP/inspector tell a
    // non-CDP engine must not exhibit (master plan §4 Phase 1 G9).
    // Descriptor inspection never runs the accessor; we only use the
    // value of a data descriptor.
    function _safeOwn(o, k) {
        try {
            let p = o;
            while (p !== null && p !== undefined) {
                const d = Object.getOwnPropertyDescriptor(p, k);
                if (d) return ("value" in d) ? d.value : undefined;
                p = Object.getPrototypeOf(p);
            }
        } catch (_) {}
        return undefined;
    }

    function _stringify(arg) {
        try {
            if (arg === undefined) return "[undefined]";
            if (arg === null) return "[null]";
            const type = typeof arg;
            if (type !== "object") return `[${type}] ${String(arg)}`;

            const ctorFn = _safeOwn(arg, "constructor");
            const ctor = (typeof ctorFn === "function" &&
                typeof ctorFn.name === "string" && ctorFn.name)
                ? ctorFn.name : "Object";
            if (arg instanceof Error) {
                const nm = _safeOwn(arg, "name");
                const msg = _safeOwn(arg, "message");
                const stk = _safeOwn(arg, "stack");
                return `[Error:${ctor}] ${nm}: ${msg}` +
                    (stk !== undefined ? `\n${stk}` : "");
            }
            if (ctor === "DOMException") {
                const nm = _safeOwn(arg, "name");
                const code = _safeOwn(arg, "code");
                const msg = _safeOwn(arg, "message");
                const stk = _safeOwn(arg, "stack");
                return `[DOMException] ${nm} (${code}): ${msg}` +
                    (stk !== undefined ? `\n${stk}` : "");
            }
            try {
                return `[Object:${ctor}] ${JSON.stringify(arg)}`;
            } catch (e) {
                return `[Object:${ctor}] (non-serializable: ${String(arg)})`;
            }
        } catch (e) {
            return `[StringifyError] ${e.message}`;
        }
    }

    globalThis.console = {
        log(...args) {
            core.ops.op_console_log(args.map(_stringify).join(" "));
        },
        warn(...args) {
            core.ops.op_console_warn(args.map(_stringify).join(" "));
        },
        error(...args) {
            core.ops.op_console_error(args.map(_stringify).join(" "));
        },
        info(...args) {
            core.ops.op_console_log(args.map(_stringify).join(" "));
        },
        debug(...args) {
            core.ops.op_console_log(args.map(_stringify).join(" "));
        },
        dir() {},
        dirxml() {},
        trace() {},
        group() {},
        groupCollapsed() {},
        groupEnd() {},
        clear() {},
        count() {},
        countReset() {},
        assert(cond, ...args) {
            if (!cond) {
                core.ops.op_console_error("Assertion failed: " + args.map(String).join(" "));
            }
        },
        table() {},
        time() {},
        timeLog() {},
        timeEnd() {},
    };
    // Native-masking of these methods is applied by stealth_bootstrap.js
    // (concatenated AFTER this file in the V8 snapshot, where
    // _maskAsNative is defined). Doing it here would no-op because
    // _maskAsNative does not exist yet at this point in the snapshot.
})(globalThis);
