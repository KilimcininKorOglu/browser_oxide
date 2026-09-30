((globalThis) => {
    const ops = Deno.core.ops;
    const _nativeTag = Symbol.for('__browser_oxide_native__');
    
    // Helpers used by various bootstraps. Ensure they exist in Workers too.
    const _defNav = (name, get) => Object.defineProperty(globalThis.navigator, name, { get, enumerable: true, configurable: true });
    const _defProtoGetter = (proto, name, get) => Object.defineProperty(proto, name, { get, enumerable: true, configurable: true });
    const _defProtoMethod = (proto, name, value) => {
        Object.defineProperty(proto, name, { value, writable: true, enumerable: true, configurable: true });
        _maskAsNative(proto, name);
    };
    const _maskAsNative = globalThis._maskAsNative || ((...args) => {
        for (const item of args) {
            if (typeof item === 'function') {
                try {
                    Object.defineProperty(item, 'toString', {
                        value: function toString() { return `function ${item.name || ''}() { [native code] }`; },
                        configurable: true
                    });
                } catch (_) {}
            } else if (item && typeof item === 'object') {
                for (const key of Object.getOwnPropertyNames(item)) {
                    if (typeof item[key] === 'function') _maskAsNative(item[key]);
                }
            }
        }
    });
    const _p = (key, fallback) => {
        if (ops.op_has_stealth_profile && ops.op_has_stealth_profile()) {
            const v = ops.op_get_profile_value(key);
            return v !== "" ? v : fallback;
        }
        return fallback;
    };

    // ================================================================
    // DOMException — Chrome-shaped.
    // ================================================================
    if (!globalThis.DOMException) {
        globalThis.DOMException = class DOMException extends Error {
            constructor(message = "", name = "Error") {
                super(message);
                this.name = name;
                this.code = 0;
            }
        };
        Object.defineProperty(globalThis.DOMException.prototype, Symbol.toStringTag, { value: "DOMException", configurable: true });
        _maskAsNative(globalThis.DOMException);
    }

    // ================================================================
    // atob / btoa — Chrome-shaped.
    // ================================================================
    if (!globalThis.atob) {
        globalThis.atob = function atob(s) {
            if (arguments.length === 0) throw new TypeError("Failed to execute 'atob' on 'Window': 1 argument required, but only 0 present.");
            const input = String(s).replace(/[\t\n\f\r ]/g, "");
            if (input.length === 0) return "";
            const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let out = "";
            for (let i = 0; i < input.length; i += 4) {
                const a = chars.indexOf(input[i]), b = chars.indexOf(input[i+1]);
                const c = chars.indexOf(input[i+2]), d = chars.indexOf(input[i+3]);
                out += String.fromCharCode((a << 2) | (b >> 4));
                if (c !== -1 && c !== 64) out += String.fromCharCode(((b & 15) << 4) | (c >> 2));
                if (d !== -1 && d !== 64) out += String.fromCharCode(((c & 3) << 6) | d);
            }
            return out;
        };
        _maskAsNative(globalThis.atob);
    }
    if (!globalThis.btoa) {
        globalThis.btoa = function btoa(s) {
            if (arguments.length === 0) throw new TypeError("Failed to execute 'btoa' on 'Window': 1 argument required, but only 0 present.");
            const str = String(s);
            for (let i = 0; i < str.length; i++) {
                if (str.charCodeAt(i) > 255) throw new DOMException("Failed to execute 'btoa' on 'Window': The string to be encoded contains characters outside of the Latin1 range.", "InvalidCharacterError");
            }
            const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let out = "";
            for (let i = 0; i < str.length; i += 3) {
                const a = str.charCodeAt(i), b = str.charCodeAt(i+1), c = str.charCodeAt(i+2);
                out += chars[a >> 2] + chars[((a & 3) << 4) | (b >> 4)];
                out += (isNaN(b) ? "=" : chars[((b & 15) << 2) | (c >> 6)]);
                out += (isNaN(c) ? "=" : chars[c & 63]);
            }
            return out;
        };
        _maskAsNative(globalThis.btoa);
    }

    // ================================================================
    // Crypto / SubtleCrypto classes + prototype.
    // ================================================================
    class Crypto {}
    globalThis.Crypto = Crypto;
    const _CryptoProto = Crypto.prototype;

    class SubtleCrypto {}
    globalThis.SubtleCrypto = SubtleCrypto;
    const _SubtleProto = SubtleCrypto.prototype;
    const _subtleInstance = Object.create(_SubtleProto);

    const _toBytes = (src) => {
        if (src == null) return new Uint8Array(0);
        if (src instanceof Uint8Array) return src;
        if (src instanceof ArrayBuffer) return new Uint8Array(src);
        if (ArrayBuffer.isView(src)) return new Uint8Array(src.buffer, src.byteOffset, src.byteLength);
        return new Uint8Array(src);
    };

    _defProtoMethod(_SubtleProto, 'digest', function digest(algorithm, data) {
        try {
            const algName = typeof algorithm === 'string' ? algorithm : (algorithm && algorithm.name) || "";
            const bytes = _toBytes(data);
            const out = ops.op_crypto_digest(String(algName), bytes);
            return Promise.resolve(out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength));
        } catch (e) { return Promise.reject(e); }
    });
    // HMAC and raw-key paths — real implementations over the Rust ops,
    // identical to the window bootstrap's (workers get the same surface).
    if (typeof __oxCryptoKeys === "undefined") { var __oxCryptoKeys = new Map(); }
    let _keySeq = 0;
    const _hashName = (alg) => {
        const name = typeof alg === 'string' ? alg : (alg && alg.name) || "";
        return String(name).toUpperCase().replace(/-/g, '');
    };
    const _algoName = (alg) => {
        const name = typeof alg === 'string' ? alg : (alg && alg.name) || "";
        return String(name).toUpperCase();
    };
    class CryptoKey {
        constructor(handle, algorithm, extractable, usages) {
            Object.defineProperties(this, {
                type: { value: 'secret', enumerable: true },
                extractable: { value: extractable, enumerable: true },
                usages: { value: Object.freeze(usages.slice()), enumerable: true },
                algorithm: { value: algorithm, enumerable: true },
            });
            Object.defineProperty(this, Symbol.toStringTag, { value: "CryptoKey" });
            Object.defineProperty(this, '__oxHandle', { value: handle });
        }
    }
    globalThis.CryptoKey = CryptoKey;

    _defProtoMethod(_SubtleProto, 'importKey', function importKey(format, keyData, algorithm, extractable, keyUsages) {
        try {
            const fmt = String(format).toLowerCase();
            let raw;
            if (fmt === 'raw' || fmt === 'pkcs8' || fmt === 'spki') {
                raw = _toBytes(keyData);
            } else if (fmt === 'jwk') {
                const b64u = (s) => {
                    s = String(s).replace(/-/g, '+').replace(/_/g, '/');
                    while (s.length % 4) s += '=';
                    const bin = globalThis.atob(s);
                    const u8 = new Uint8Array(bin.length);
                    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
                    return u8;
                };
                const jwk = keyData;
                raw = jwk && jwk.k ? b64u(jwk.k) : jwk && jwk.n ? b64u(jwk.n) : new Uint8Array(0);
            } else {
                return Promise.reject(new DOMException(`unsupported key format ${format}`, "NotSupportedError"));
            }
            const algObj = typeof algorithm === 'string' ? { name: algorithm } : (algorithm || {});
            const algName = String(algObj.name || '').toUpperCase();
            let hashName = '';
            if (algObj.hash) {
                hashName = typeof algObj.hash === 'string'
                    ? _hashName(algObj.hash)
                    : _hashName(algObj.hash.name || '');
            }
            const handle = ++_keySeq;
            __oxCryptoKeys.set(handle, { raw, algName, hashName });
            const usages = Array.from(keyUsages || []);
            const algExposed = algName === 'HMAC'
                ? { name: 'HMAC', hash: { name: hashName === 'SHA1' ? 'SHA-1' : hashName === 'SHA384' ? 'SHA-384' : hashName === 'SHA512' ? 'SHA-512' : 'SHA-256' } }
                : { name: algName };
            return Promise.resolve(new CryptoKey(handle, algExposed, extractable !== false, usages));
        } catch (e) { return Promise.reject(e); }
    });
    _defProtoMethod(_SubtleProto, 'exportKey', function exportKey(format, key) {
        try {
            const entry = __oxCryptoKeys.get(key && key.__oxHandle);
            if (!entry) return Promise.reject(new DOMException("key not found", "InvalidAccessError"));
            if (String(format).toLowerCase() !== 'raw') {
                return Promise.reject(new DOMException(`unsupported export format ${format}`, "NotSupportedError"));
            }
            return Promise.resolve(entry.raw.slice().buffer);
        } catch (e) { return Promise.reject(e); }
    });
    const _hmacHash = (hashNoDash) =>
        ({ 'SHA1': 'SHA-1', 'SHA256': 'SHA-256', 'SHA384': 'SHA-384', 'SHA512': 'SHA-512' })[hashNoDash] || 'SHA-256';
    _defProtoMethod(_SubtleProto, 'sign', function sign(algorithm, key, data) {
        try {
            const algName = _algoName(algorithm);
            const entry = __oxCryptoKeys.get(key && key.__oxHandle);
            if (!entry) return Promise.reject(new DOMException("key not found", "InvalidAccessError"));
            if (algName === 'HMAC') {
                const hash = entry.hashName
                    ? _hmacHash(entry.hashName)
                    : _hmacHash(_hashName(typeof algorithm === 'object' && algorithm && algorithm.hash || ''));
                const out = ops.op_crypto_hmac_sign(hash, entry.raw, _toBytes(data));
                return Promise.resolve(out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength));
            }
            return Promise.reject(new DOMException(`${algName} not implemented`, "NotSupportedError"));
        } catch (e) { return Promise.reject(e); }
    });
    _defProtoMethod(_SubtleProto, 'verify', function verify(algorithm, key, signature, data) {
        try {
            return Promise.resolve(this.sign(algorithm, key, data)).then((tag) => {
                const a = new Uint8Array(tag), b = _toBytes(signature);
                if (a.length !== b.length) return false;
                let diff = 0;
                for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
                return diff === 0;
            });
        } catch (e) { return Promise.reject(e); }
    });
    const _subtleNotImplemented = (name) => function (...args) {
        return Promise.reject(new DOMException(`${name} not implemented`, "NotSupportedError"));
    };
    for (const m of ['encrypt','decrypt','generateKey','deriveKey','deriveBits','wrapKey','unwrapKey']) {
        _defProtoMethod(_SubtleProto, m, _subtleNotImplemented(m));
    }

    _defProtoMethod(_CryptoProto, 'getRandomValues', function getRandomValues(arr) {
        if (!ArrayBuffer.isView(arr)) throw new TypeError("getRandomValues expects an ArrayBufferView");
        if (arr.byteLength > 65536) throw new DOMException("QuotaExceededError", "QuotaExceededError");
        const u8 = new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
        ops.op_crypto_random_fill(u8);
        return arr;
    });
    _defProtoMethod(_CryptoProto, 'randomUUID', function randomUUID() {
        const b = new Uint8Array(16);
        ops.op_crypto_random_fill(b);
        b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
        const hex = [];
        for (let i = 0; i < 16; i++) hex.push(b[i].toString(16).padStart(2, '0'));
        return `${hex.slice(0,4).join('')}-${hex.slice(4,6).join('')}-${hex.slice(6,8).join('')}-${hex.slice(8,10).join('')}-${hex.slice(10,16).join('')}`;
    });
    _defProtoGetter(_CryptoProto, 'subtle', () => _subtleInstance);

    Object.defineProperty(_CryptoProto, Symbol.toStringTag, { value: "Crypto", configurable: true });
    Object.defineProperty(_SubtleProto, Symbol.toStringTag, { value: "SubtleCrypto", configurable: true });

    globalThis.crypto = Object.create(_CryptoProto);

    // ================================================================
    // TextEncoder / TextDecoder.
    // ================================================================
    if (!globalThis.TextEncoder || !TextEncoder.prototype.encodeInto) {
        class TextEncoder {
            constructor() {}
            encode(str) {
                str = String(str == null ? "" : str);
                const buf = [];
                for (let i = 0; i < str.length; i++) {
                    let c = str.charCodeAt(i);
                    if (c >= 0xD800 && c <= 0xDBFF && i + 1 < str.length) {
                        const low = str.charCodeAt(i + 1);
                        if (low >= 0xDC00 && low <= 0xDFFF) { c = 0x10000 + ((c - 0xD800) << 10) + (low - 0xDC00); i++; }
                    }
                    if (c < 0x80) buf.push(c);
                    else if (c < 0x800) buf.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
                    else if (c < 0x10000) buf.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
                    else buf.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 0x3f), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
                }
                return new Uint8Array(buf);
            }
            encodeInto(source, destination) {
                if (!(destination instanceof Uint8Array)) throw new TypeError("encodeInto destination must be a Uint8Array");
                source = String(source == null ? "" : source);
                let read = 0, written = 0;
                for (let i = 0; i < source.length; i++) {
                    let c = source.charCodeAt(i);
                    let extraChar = 0;
                    if (c >= 0xD800 && c <= 0xDBFF && i + 1 < source.length) {
                        const low = source.charCodeAt(i + 1);
                        if (low >= 0xDC00 && low <= 0xDFFF) { c = 0x10000 + ((c - 0xD800) << 10) + (low - 0xDC00); extraChar = 1; }
                    }
                    let bytes;
                    if (c < 0x80) bytes = [c];
                    else if (c < 0x800) bytes = [0xc0 | (c >> 6), 0x80 | (c & 0x3f)];
                    else if (c < 0x10000) bytes = [0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f)];
                    else bytes = [0xf0 | (c >> 18), 0x80 | ((c >> 12) & 0x3f), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f)];
                    if (written + bytes.length > destination.length) break;
                    for (let j = 0; j < bytes.length; j++) destination[written + j] = bytes[j];
                    written += bytes.length; read += 1 + extraChar; if (extraChar) i++;
                }
                return { read, written };
            }
        }
        globalThis.TextEncoder = TextEncoder;
        _defProtoGetter(TextEncoder.prototype, 'encoding', () => "utf-8");
        _defProtoMethod(TextEncoder.prototype, 'encode', TextEncoder.prototype.encode);
        _defProtoMethod(TextEncoder.prototype, 'encodeInto', TextEncoder.prototype.encodeInto);
        _maskAsNative(TextEncoder);
    }
    if (!globalThis.TextDecoder || !('encoding' in TextDecoder.prototype)) {
        class TextDecoder {
            constructor(label = "utf-8", options = {}) {
                this._label = String(label).toLowerCase(); this._fatal = !!options.fatal; this._ignoreBOM = !!options.ignoreBOM;
            }
            decode(buf, options) {
                if (buf === undefined) return "";
                let bytes = _toBytes(buf);
                let str = "", i = 0;
                if (!this._ignoreBOM && bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) i = 3;
                while (i < bytes.length) {
                    const b0 = bytes[i];
                    if (b0 < 0x80) { str += String.fromCharCode(b0); i++; }
                    else if ((b0 & 0xe0) === 0xc0 && i + 1 < bytes.length) { const cp = ((b0 & 0x1f) << 6) | (bytes[i+1] & 0x3f); str += String.fromCharCode(cp); i += 2; }
                    else if ((b0 & 0xf0) === 0xe0 && i + 2 < bytes.length) { const cp = ((b0 & 0x0f) << 12) | ((bytes[i+1] & 0x3f) << 6) | (bytes[i+2] & 0x3f); str += String.fromCharCode(cp); i += 3; }
                    else if ((b0 & 0xf8) === 0xf0 && i + 3 < bytes.length) {
                        let cp = ((b0 & 0x07) << 18) | ((bytes[i+1] & 0x3f) << 12) | ((bytes[i+2] & 0x3f) << 6) | (bytes[i+3] & 0x3f);
                        cp -= 0x10000; str += String.fromCharCode(0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3ff)); i += 4;
                    }
                    else { if (this._fatal) throw new TypeError("The encoded data was not valid."); str += "\uFFFD"; i++; }
                }
                return str;
            }
        }
        globalThis.TextDecoder = TextDecoder;
        _defProtoGetter(TextDecoder.prototype, 'encoding', function encoding() { return this._label || "utf-8"; });
        _defProtoGetter(TextDecoder.prototype, 'fatal', function fatal() { return this._fatal; });
        _defProtoGetter(TextDecoder.prototype, 'ignoreBOM', function ignoreBOM() { return this._ignoreBOM; });
        _defProtoMethod(TextDecoder.prototype, 'decode', TextDecoder.prototype.decode);
        _maskAsNative(TextDecoder);
    }

    // ================================================================
    // URL / URLSearchParams.
    // ================================================================
    // Always install our polyfills. V8 may expose half-initialized native
    // URLSearchParams/URL bindings that throw "Illegal constructor" —
    // forcing our pure-JS implementations guarantees constructability.
    {
        // application/x-www-form-urlencoded (URL Standard §5.1): a space is "+",
        // and only ASCII alphanumerics and *-._ stay unescaped.
        const _formEncode = (s) => encodeURIComponent(String(s).toWellFormed())
            .replace(/[!'()~]/g, (c) => '%' + c.charCodeAt(0).toString(16).toUpperCase())
            .replace(/%20/g, '+');
        const _formDecode = (s) => s.replace(/\+/g, ' ').replace(/(?:%[0-9a-fA-F]{2})+/g, (run) => {
            const bytes = new Uint8Array(run.length / 3);
            for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(run.substr(i * 3 + 1, 2), 16);
            return new TextDecoder().decode(bytes);
        });
        const _parseQuery = (input) => {
            const list = [];
            for (const seq of input.split('&')) {
                if (seq === '') continue;
                const eq = seq.indexOf('=');
                list.push([_formDecode(eq < 0 ? seq : seq.slice(0, eq)), _formDecode(eq < 0 ? '' : seq.slice(eq + 1))]);
            }
            return list;
        };
        const _need = (args, count, cls, method) => {
            if (args.length < count) {
                throw new TypeError(`Failed to execute '${method}' on '${cls}': ${count} argument${count > 1 ? 's' : ''} required, but only ${args.length} present.`);
            }
        };
        const _pairFrom = (item) => {
            const pair = [...item];
            if (pair.length !== 2) {
                throw new TypeError("Failed to construct 'URLSearchParams': Each query pair must be an iterable [name, value] tuple");
            }
            return [String(pair[0]), String(pair[1])];
        };
        const _listFrom = (init) => {
            if (init === null || init === undefined) return [];
            if (typeof init !== 'object' && typeof init !== 'function') {
                return _parseQuery(String(init).replace(/^\?/, ''));
            }
            if (typeof init[Symbol.iterator] === 'function') return [...init].map(_pairFrom);
            return Object.keys(init).map((k) => [String(k), String(init[k])]);
        };
        // list and, once URL.searchParams links it, the owner's update callback.
        const _uspState = new WeakMap();
        const _uspOf = (self) => {
            const st = _uspState.get(self);
            if (!st) throw new TypeError('Illegal invocation');
            return st;
        };
        const _uspSerialize = (list) => list.map(([k, v]) => _formEncode(k) + '=' + _formEncode(v)).join('&');
        const _uspUpdate = (st) => { if (st.onChange) st.onChange(_uspSerialize(st.list)); };
        const _uspIterator = (self, pick) => {
            const st = _uspOf(self);
            let i = 0;
            return {
                next() { return i < st.list.length ? { value: pick(st.list[i++]), done: false } : { value: undefined, done: true }; },
                [Symbol.iterator]() { return this; },
            };
        };
        class URLSearchParams {
            constructor(init = undefined) {
                _uspState.set(this, { list: _listFrom(init), onChange: null });
            }
            get size() { return _uspOf(this).list.length; }
            append(name, value) {
                _need(arguments, 2, 'URLSearchParams', 'append');
                const st = _uspOf(this);
                st.list.push([String(name), String(value)]);
                _uspUpdate(st);
            }
            delete(name, value = undefined) {
                _need(arguments, 1, 'URLSearchParams', 'delete');
                const st = _uspOf(this);
                const key = String(name);
                const val = value === undefined ? undefined : String(value);
                st.list = st.list.filter(([k, v]) => k !== key || (val !== undefined && v !== val));
                _uspUpdate(st);
            }
            get(name) {
                _need(arguments, 1, 'URLSearchParams', 'get');
                const hit = _uspOf(this).list.find(([k]) => k === String(name));
                return hit ? hit[1] : null;
            }
            getAll(name) {
                _need(arguments, 1, 'URLSearchParams', 'getAll');
                return _uspOf(this).list.filter(([k]) => k === String(name)).map(([, v]) => v);
            }
            has(name, value = undefined) {
                _need(arguments, 1, 'URLSearchParams', 'has');
                const val = value === undefined ? undefined : String(value);
                return _uspOf(this).list.some(([k, v]) => k === String(name) && (val === undefined || v === val));
            }
            set(name, value) {
                _need(arguments, 2, 'URLSearchParams', 'set');
                const st = _uspOf(this);
                const key = String(name);
                const val = String(value);
                const first = st.list.findIndex(([k]) => k === key);
                if (first < 0) st.list.push([key, val]);
                else {
                    st.list[first][1] = val;
                    st.list = st.list.filter(([k], i) => k !== key || i === first);
                }
                _uspUpdate(st);
            }
            sort() {
                const st = _uspOf(this);
                const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
                st.list = st.list.map((p, i) => [p, i]).sort((a, b) => cmp(a[0][0], b[0][0]) || a[1] - b[1]).map(([p]) => p);
                _uspUpdate(st);
            }
            toString() { return _uspSerialize(_uspOf(this).list); }
            forEach(callback, thisArg = undefined) {
                _need(arguments, 1, 'URLSearchParams', 'forEach');
                if (typeof callback !== 'function') {
                    throw new TypeError("Failed to execute 'forEach' on 'URLSearchParams': parameter 1 is not of type 'Function'.");
                }
                const st = _uspOf(this);
                for (let i = 0; i < st.list.length; i++) callback.call(thisArg, st.list[i][1], st.list[i][0], this);
            }
            keys() { return _uspIterator(this, ([k]) => k); }
            values() { return _uspIterator(this, ([, v]) => v); }
            entries() { return _uspIterator(this, ([k, v]) => [k, v]); }
            [Symbol.iterator]() { return this.entries(); }
        }
        // Chrome lists these on the prototype as enumerable, and never marks them as script.
        const _exposeProto = (cls, tag) => {
            const proto = cls.prototype;
            for (const name of Object.getOwnPropertyNames(proto)) {
                if (name === 'constructor') continue;
                const d = Object.getOwnPropertyDescriptor(proto, name);
                d.enumerable = true;
                Object.defineProperty(proto, name, d);
                _maskAsNative(proto, name);
            }
            Object.defineProperty(proto, Symbol.toStringTag, { value: tag, configurable: true });
            _maskAsNative(proto, Symbol.iterator);
            _maskAsNative(cls);
        };
        _exposeProto(URLSearchParams, 'URLSearchParams');
        try {
            Object.defineProperty(globalThis, 'URLSearchParams', { value: URLSearchParams, writable: true, configurable: true, enumerable: false });
        } catch (_) { globalThis.URLSearchParams = URLSearchParams; }

        // The URL class holds no parsing logic: op_url_parse and op_url_set run the
        // URL Standard's parser and setters, and answer the eleven components in
        // this order.
        const [HREF, ORIGIN, PROTOCOL, USERNAME, PASSWORD, HOST, HOSTNAME, PORT, PATHNAME, SEARCH, HASH] =
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        const _urlState = new WeakMap();
        const _urlOf = (self) => {
            const st = _urlState.get(self);
            if (!st) throw new TypeError('Illegal invocation');
            return st;
        };
        const _parseUrl = (input, base) => (base === undefined
            ? ops.op_url_parse(input, '', false)
            : ops.op_url_parse(input, base, true));
        const _resetParams = (st) => {
            if (st.params) _uspState.get(st.params).list = _parseQuery(st.parts[SEARCH].replace(/^\?/, ''));
        };
        const _setUrlPart = (self, name, value) => {
            const st = _urlOf(self);
            st.parts = ops.op_url_set(st.parts[HREF], name, String(value));
            if (name === 'search') _resetParams(st);
        };
        const _urlAccessors = {
            origin: ORIGIN, protocol: PROTOCOL, username: USERNAME, password: PASSWORD, host: HOST,
            hostname: HOSTNAME, port: PORT, pathname: PATHNAME, search: SEARCH, hash: HASH,
        };
        globalThis.URL = class URL {
            constructor(url, base = undefined) {
                if (arguments.length === 0) {
                    throw new TypeError("Failed to construct 'URL': 1 argument required, but only 0 present.");
                }
                const parts = _parseUrl(String(url), base === undefined ? undefined : String(base));
                if (!parts) throw new TypeError("Failed to construct 'URL': Invalid URL");
                _urlState.set(this, { parts, params: null });
            }
            static canParse(url, base = undefined) {
                if (arguments.length === 0) {
                    throw new TypeError("Failed to execute 'canParse' on 'URL': 1 argument required, but only 0 present.");
                }
                return _parseUrl(String(url), base === undefined ? undefined : String(base)) !== null;
            }
            static parse(url, base = undefined) {
                if (arguments.length === 0) {
                    throw new TypeError("Failed to execute 'parse' on 'URL': 1 argument required, but only 0 present.");
                }
                const parts = _parseUrl(String(url), base === undefined ? undefined : String(base));
                if (!parts) return null;
                const u = Object.create(globalThis.URL.prototype);
                _urlState.set(u, { parts, params: null });
                return u;
            }
            get href() { return _urlOf(this).parts[HREF]; }
            set href(value) {
                const st = _urlOf(this);
                const parts = _parseUrl(String(value), undefined);
                if (!parts) throw new TypeError("Failed to set the 'href' property on 'URL': Invalid URL");
                st.parts = parts;
                _resetParams(st);
            }
            get searchParams() {
                const st = _urlOf(this);
                if (!st.params) {
                    st.params = new URLSearchParams(st.parts[SEARCH]);
                    _uspState.get(st.params).onChange = (serialized) => {
                        st.parts = ops.op_url_set(st.parts[HREF], 'search', serialized);
                    };
                }
                return st.params;
            }
            toString() { return _urlOf(this).parts[HREF]; }
            toJSON() { return _urlOf(this).parts[HREF]; }
            static createObjectURL(obj) {
                const u = 'blob:' + (globalThis.location && globalThis.location.origin || 'null') + '/' + _randomUUID();
                let data, contentType = '';
                if (obj && obj._data instanceof Uint8Array) { data = obj._data; contentType = String(obj.type || ''); }
                else if (obj instanceof Uint8Array) data = obj;
                else if (typeof obj === 'string') data = new TextEncoder().encode(obj);
                else data = new Uint8Array();
                try { ops.op_blob_register(u, data, contentType); } catch (e) {}
                return u;
            }
            static revokeObjectURL(url) { try { ops.op_blob_revoke(url); } catch (e) {} }
        };
        for (const [name, index] of Object.entries(_urlAccessors)) {
            const setter = name === 'origin' ? undefined : function (value) { _setUrlPart(this, name, value); };
            Object.defineProperty(globalThis.URL.prototype, name, {
                get() { return _urlOf(this).parts[index]; }, set: setter, enumerable: true, configurable: true,
            });
        }
        _exposeProto(globalThis.URL, 'URL');
        _maskAsNative(globalThis.URL, 'canParse', 'parse', 'createObjectURL', 'revokeObjectURL');
    }
    function _randomUUID() {
        if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') { try { return globalThis.crypto.randomUUID(); } catch (e) {} }
        return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, c => {
            const r = Math.random() * 16 | 0; return (c === 'x' ? r : (r & 0x3 | 0x8)).toString(16);
        });
    }

    // ================================================================
    // Blob / File.
    // ================================================================
    if (!globalThis.Blob) {
        const _encoder = new TextEncoder();
        const _decoder = new TextDecoder();
        globalThis.Blob = class Blob {
            constructor(parts = [], options = {}) {
                this.type = options.type || "";
                const arrays = parts.map(p => {
                    if (typeof p === "string") return _encoder.encode(p);
                    if (p instanceof ArrayBuffer) return new Uint8Array(p);
                    if (ArrayBuffer.isView(p)) return new Uint8Array(p.buffer, p.byteOffset, p.byteLength);
                    if (p instanceof Blob) return p._data;
                    return _encoder.encode(String(p));
                });
                const totalLen = arrays.reduce((s, a) => s + a.byteLength, 0);
                const merged = new Uint8Array(totalLen);
                let offset = 0;
                for (const a of arrays) { merged.set(a, offset); offset += a.byteLength; }
                this._data = merged; this.size = totalLen;
            }
            text() { return Promise.resolve(_decoder.decode(this._data)); }
            arrayBuffer() { return Promise.resolve(this._data.buffer.slice(this._data.byteOffset, this._data.byteOffset + this._data.byteLength)); }
            slice(start = 0, end = this.size, type = "") {
                const sliced = this._data.slice(start, end);
                const b = new Blob([], { type }); b._data = sliced; b.size = sliced.byteLength; return b;
            }
            // P2b — Blob.prototype.stream(). Was missing; SPA state persisters
            // (e.g. duolingo: `new Blob([...]).stream().pipeThrough(new
            // CompressionStream('gzip'))`) threw `.stream is not a function`,
            // aborting the event-loop drain before React's commit → empty #root.
            stream() {
                const data = this._data;
                if (typeof globalThis.ReadableStream === 'function') {
                    return new globalThis.ReadableStream({
                        start(controller) {
                            if (data && data.byteLength) controller.enqueue(data);
                            controller.close();
                        }
                    });
                }
                // Minimal fallback if ReadableStream is unavailable.
                let done = false;
                return { getReader() { return { read() { if (done) return Promise.resolve({ done: true, value: undefined }); done = true; return Promise.resolve({ done: false, value: data }); }, releaseLock() {}, cancel() { return Promise.resolve(); } }; } };
            }
        };
        _maskAsNative(globalThis.Blob);
    }
    if (!globalThis.File) {
        globalThis.File = class File extends Blob {
            constructor(parts, name, options = {}) { super(parts, options); this.name = name; this.lastModified = options.lastModified || Date.now(); }
        };
        _maskAsNative(globalThis.File);
    }

    // ================================================================
    // FormData.
    // ================================================================
    if (!globalThis.FormData) {
        globalThis.FormData = class FormData {
            #data;
            constructor() { this.#data = []; }
            append(name, value) { this.#data.push([String(name), value]); }
            delete(name) { this.#data = this.#data.filter(([k]) => k !== name); }
            get(name) { const p = this.#data.find(([k]) => k === name); return p ? p[1] : null; }
            getAll(name) { return this.#data.filter(([k]) => k === name).map(([, v]) => v); }
            has(name) { return this.#data.some(([k]) => k === name); }
            set(name, value) { this.delete(name); this.append(name, value); }
            forEach(cb, thisArg) { for (const [k, v] of this.#data) cb.call(thisArg, v, k, this); }
            keys() { return this.#data.map(([k]) => k)[Symbol.iterator](); }
            values() { return this.#data.map(([, v]) => v)[Symbol.iterator](); }
            entries() { return this.#data[Symbol.iterator](); }
            [Symbol.iterator]() { return this.entries(); }
        };
        _maskAsNative(globalThis.FormData);
    }

    // ================================================================
    // AbortController / AbortSignal.
    // ================================================================
    if (!globalThis.AbortController) {
        class AbortSignal {
            constructor() { this.aborted = false; this.reason = undefined; this._listeners = []; }
            addEventListener(type, cb) { if (type === "abort") this._listeners.push(cb); }
            removeEventListener(type, cb) { if (type === "abort") this._listeners = this._listeners.filter(l => l !== cb); }
            throwIfAborted() { if (this.aborted) throw this.reason; }
            static abort(reason) { const sig = new AbortSignal(); sig.aborted = true; sig.reason = reason || new DOMException("The operation was aborted.", "AbortError"); return sig; }
            static timeout(ms) {
                const sig = new AbortSignal();
                setTimeout(() => {
                    sig.aborted = true; sig.reason = new DOMException("The operation timed out.", "TimeoutError");
                    for (const cb of sig._listeners) cb();
                }, ms);
                return sig;
            }
        }
        class AbortController {
            constructor() { this.signal = new AbortSignal(); }
            abort(reason) {
                if (this.signal.aborted) return;
                this.signal.aborted = true;
                this.signal.reason = reason || new DOMException("The operation was aborted.", "AbortError");
                for (const cb of this.signal._listeners) cb();
            }
        }
        globalThis.AbortController = AbortController;
        globalThis.AbortSignal = AbortSignal;
        _maskAsNative(AbortController, AbortSignal);
    }

    // ================================================================
    // OffscreenCanvas.
    // ================================================================
    if (!globalThis.OffscreenCanvas) {
        class OffscreenCanvas {
            constructor(width, height) { this.width = width | 0; this.height = height | 0; }
            getContext(_type, _opts) { return null; }
            transferToImageBitmap() { return { width: this.width, height: this.height, close() {} }; }
            convertToBlob(options) { return Promise.resolve(new Blob([], { type: (options && options.type) || "image/png" })); }
        }
        Object.defineProperty(OffscreenCanvas.prototype, Symbol.toStringTag, { value: "OffscreenCanvas", configurable: true, });
        globalThis.OffscreenCanvas = OffscreenCanvas;
        _maskAsNative(OffscreenCanvas);
    }

    // ================================================================
    // IndexedDB — In-memory stub.
    // ================================================================
    if (!globalThis.indexedDB || !globalThis.indexedDB._browserOxideReal) {
        const _dbRegistry = new Map();
        function _clone(v) {
            if (typeof globalThis.structuredClone === "function") { try { return globalThis.structuredClone(v); } catch (_e) {} }
            try { return JSON.parse(JSON.stringify(v)); } catch (_e) { return v; }
        }
        function _keyCmp(a, b) {
            if (typeof a === typeof b) { if (a < b) return -1; if (a > b) return 1; return 0; }
            if (typeof a === "number") return -1; if (typeof b === "number") return 1; return 0;
        }
        function _extractKey(value, keyPath) {
            if (!keyPath) return undefined;
            if (Array.isArray(keyPath)) return keyPath.map((p) => value?.[p]);
            const parts = String(keyPath).split("."); let cur = value;
            for (const p of parts) { if (cur == null) return undefined; cur = cur[p]; }
            return cur;
        }
        class IDBRequest {
            constructor(source) { this.result = undefined; this.error = null; this.source = source || null; this.transaction = null; this.readyState = "pending"; this.onsuccess = null; this.onerror = null; this._listeners = { success: [], error: [] }; }
            addEventListener(type, listener) { if (!this._listeners[type]) this._listeners[type] = []; this._listeners[type].push(listener); }
            removeEventListener(type, listener) { const arr = this._listeners[type]; if (!arr) return; const i = arr.indexOf(listener); if (i >= 0) arr.splice(i, 1); }
            _fireSuccess() { this.readyState = "done"; queueMicrotask(() => { const ev = { target: this, type: "success" }; if (typeof this.onsuccess === "function") try { this.onsuccess(ev); } catch (_e) {} for (const l of (this._listeners.success || []).slice()) try { l.call(this, ev); } catch (_e) {} }); }
            _fireError(err) { this.readyState = "done"; this.error = err; queueMicrotask(() => { const ev = { target: this, type: "error" }; if (typeof this.onerror === "function") try { this.onerror(ev); } catch (_e) {} for (const l of (this._listeners.error || []).slice()) try { l.call(this, ev); } catch (_e) {} }); }
        }
        class IDBOpenDBRequest extends IDBRequest { constructor() { super(); this.onupgradeneeded = null; this.onblocked = null; } }
        class IDBKeyRange {
            constructor(lower, upper, lowerOpen, upperOpen) { this.lower = lower; this.upper = upper; this.lowerOpen = !!lowerOpen; this.upperOpen = !!upperOpen; }
            includes(key) {
                if (this.lower !== undefined) { const c = _keyCmp(key, this.lower); if (c < 0) return false; if (c === 0 && this.lowerOpen) return false; }
                if (this.upper !== undefined) { const c = _keyCmp(key, this.upper); if (c > 0) return false; if (c === 0 && this.upperOpen) return false; }
                return true;
            }
            static bound(lower, upper, lowerOpen = false, upperOpen = false) { return new IDBKeyRange(lower, upper, lowerOpen, upperOpen); }
            static only(value) { return new IDBKeyRange(value, value, false, false); }
            static lowerBound(lower, open = false) { return new IDBKeyRange(lower, undefined, open, false); }
            static upperBound(upper, open = false) { return new IDBKeyRange(undefined, upper, false, open); }
        }
        class IDBCursor {
            constructor(store, range, direction) {
                this.source = store; this.direction = direction || "next"; this._store = store; this._range = range;
                this._keys = store._sortedKeys().filter((k) => !range || range.includes(k));
                if (this.direction === "prev" || this.direction === "prevunique") this._keys.reverse();
                this._idx = -1; this.key = undefined; this.primaryKey = undefined; this.value = undefined;
            }
            _advanceTo(idx) {
                this._idx = idx;
                if (idx < this._keys.length) { this.key = this._keys[idx]; this.primaryKey = this.key; this.value = _clone(this._store._data.get(this.key)); }
                else { this.key = undefined; this.primaryKey = undefined; this.value = undefined; }
            }
            ['continue'](_targetKey) { this._step(); }
            advance(count) { for (let i = 0; i < count; i++) this._step(); }
            _step() { this._advanceTo(this._idx + 1); if (this._request) { const done = this._idx >= this._keys.length; this._request.result = done ? null : this; this._request._fireSuccess(); } }
        }
        class IDBObjectStore {
            constructor(name, options, transaction) { this.name = name; this.keyPath = (options && options.keyPath) || null; this.indexNames = []; this.autoIncrement = !!(options && options.autoIncrement); this.transaction = transaction || null; this._data = new Map(); this._nextKey = 1; }
            _sortedKeys() { return [...this._data.keys()].sort(_keyCmp); }
            _resolveKey(value, explicitKey) {
                if (this.keyPath) { const extracted = _extractKey(value, this.keyPath); if (extracted !== undefined) return extracted; if (this.autoIncrement) return this._nextKey++; return undefined; }
                if (explicitKey !== undefined) return explicitKey;
                if (this.autoIncrement) return this._nextKey++; return undefined;
            }
            put(value, key) { const r = new IDBRequest(this); const resolvedKey = this._resolveKey(value, key); if (resolvedKey === undefined) { r._fireError(new Error("DataError: no key")); return r; } this._data.set(resolvedKey, _clone(value)); if (this.autoIncrement && typeof resolvedKey === "number" && resolvedKey >= this._nextKey) { this._nextKey = resolvedKey + 1; } r.result = resolvedKey; r._fireSuccess(); return r; }
            add(value, key) { const r = new IDBRequest(this); const resolvedKey = this._resolveKey(value, key); if (resolvedKey === undefined) { r._fireError(new Error("DataError: no key")); return r; } if (this._data.has(resolvedKey)) { r._fireError(new Error("ConstraintError: key exists")); return r; } this._data.set(resolvedKey, _clone(value)); r.result = resolvedKey; r._fireSuccess(); return r; }
            get(key) { const r = new IDBRequest(this); if (key instanceof IDBKeyRange) { for (const k of this._sortedKeys()) { if (key.includes(k)) { r.result = _clone(this._data.get(k)); r._fireSuccess(); return r; } } r.result = undefined; } else { const v = this._data.get(key); r.result = v === undefined ? undefined : _clone(v); } r._fireSuccess(); return r; }
            getAll(queryOrRange, count) { const r = new IDBRequest(this); const out = []; const limit = count ?? Infinity; for (const k of this._sortedKeys()) { if (out.length >= limit) break; if (queryOrRange == null) { out.push(_clone(this._data.get(k))); } else if (queryOrRange instanceof IDBKeyRange) { if (queryOrRange.includes(k)) out.push(_clone(this._data.get(k))); } else if (_keyCmp(k, queryOrRange) === 0) { out.push(_clone(this._data.get(k))); } } r.result = out; r._fireSuccess(); return r; }
            getAllKeys(queryOrRange, count) { const r = new IDBRequest(this); const out = []; const limit = count ?? Infinity; for (const k of this._sortedKeys()) { if (out.length >= limit) break; if (queryOrRange == null) { out.push(k); } else if (queryOrRange instanceof IDBKeyRange) { if (queryOrRange.includes(k)) out.push(k); } else if (_keyCmp(k, queryOrRange) === 0) { out.push(k); } } r.result = out; r._fireSuccess(); return r; }
            delete(key) { const r = new IDBRequest(this); if (key instanceof IDBKeyRange) { for (const k of this._sortedKeys()) { if (key.includes(k)) this._data.delete(k); } } else { this._data.delete(key); } r._fireSuccess(); return r; }
            clear() { const r = new IDBRequest(this); this._data.clear(); r._fireSuccess(); return r; }
            count(query) { const r = new IDBRequest(this); if (query == null) { r.result = this._data.size; } else if (query instanceof IDBKeyRange) { let n = 0; for (const k of this._data.keys()) { if (query.includes(k)) n++; } r.result = n; } else { r.result = this._data.has(query) ? 1 : 0; } r._fireSuccess(); return r; }
            openCursor(range, direction) { const r = new IDBRequest(this); let rangeObj = null; if (range instanceof IDBKeyRange) rangeObj = range; else if (range != null) rangeObj = IDBKeyRange.only(range); const cursor = new IDBCursor(this, rangeObj, direction); cursor._request = r; queueMicrotask(() => cursor._step()); return r; }
            createIndex(name) { if (!this.indexNames.includes(name)) this.indexNames.push(name); return { name, get: (k) => this.get(k), getAll: (q, c) => this.getAll(q, c), }; }
            index(name) { return { name, get: (k) => this.get(k), getAll: (q, c) => this.getAll(q, c), }; }
        }
        class IDBTransaction {
            constructor(db, storeNames, mode) { this._db = db; this._storeNames = storeNames; this.mode = mode || "readonly"; this.db = db; this.error = null; this.oncomplete = null; this.onerror = null; this.onabort = null; this._listeners = { complete: [], error: [], abort: [] }; this._active = true; queueMicrotask(() => this._complete()); }
            objectStore(name) { if (!this._db._stores.has(name)) throw new Error("NotFoundError: store " + name); const store = this._db._stores.get(name); store.transaction = this; return store; }
            commit() { this._complete(); }
            abort() { this._active = false; const ev = { target: this, type: "abort" }; if (typeof this.onabort === "function") queueMicrotask(() => { try { this.onabort(ev); } catch (_e) {} }); }
            addEventListener(type, listener) { if (!this._listeners[type]) this._listeners[type] = []; this._listeners[type].push(listener); }
            removeEventListener(type, listener) { const arr = this._listeners[type]; if (!arr) return; const i = arr.indexOf(listener); if (i >= 0) arr.splice(i, 1); }
            _complete() { if (!this._active) return; this._active = false; const ev = { target: this, type: "complete" }; if (typeof this.oncomplete === "function") try { this.oncomplete(ev); } catch (_e) {} for (const l of (this._listeners.complete || []).slice()) try { l.call(this, ev); } catch (_e) {} }
        }
        class IDBDatabase {
            constructor(name, version) { this.name = name; this.version = version; this._stores = new Map(); this.objectStoreNames = []; this.onclose = null; this.onversionchange = null; this.onabort = null; this.onerror = null; }
            createObjectStore(name, options) { if (this._stores.has(name)) throw new Error("ConstraintError: store already exists"); const store = new IDBObjectStore(name, options, null); this._stores.set(name, store); this.objectStoreNames.push(name); return store; }
            deleteObjectStore(name) { this._stores.delete(name); this.objectStoreNames = this.objectStoreNames.filter((n) => n !== name); }
            transaction(storeNames, mode) { const names = Array.isArray(storeNames) ? storeNames : [storeNames]; return new IDBTransaction(this, names, mode); }
            close() {}
        }
        class IDBFactory {
            constructor() { this._browserOxideReal = true; }
            open(name, version) {
                const req = new IDBOpenDBRequest(); const targetVersion = version || 1; let db = _dbRegistry.get(name); const oldVersion = db ? db.version : 0;
                if (!db) { db = new IDBDatabase(name, targetVersion); _dbRegistry.set(name, db); } else if (db.version < targetVersion) db.version = targetVersion;
                req.result = db;
                queueMicrotask(() => {
                    if (oldVersion < targetVersion) {
                        const tx = new IDBTransaction(db, [], "versionchange"); req.transaction = tx;
                        const ev = { target: req, oldVersion, newVersion: targetVersion, type: "upgradeneeded", };
                        if (typeof req.onupgradeneeded === "function") try { req.onupgradeneeded(ev); } catch (_e) {}
                        tx._complete(); req.transaction = null;
                    }
                    req._fireSuccess();
                });
                return req;
            }
            deleteDatabase(name) { const r = new IDBOpenDBRequest(); _dbRegistry.delete(name); r._fireSuccess(); return r; }
            databases() { return Promise.resolve([..._dbRegistry.entries()].map(([name, db]) => ({ name, version: db.version, }))); }
            cmp(a, b) { return _keyCmp(a, b); }
        }
        globalThis.indexedDB = new IDBFactory();
        globalThis.IDBFactory = IDBFactory;
        globalThis.IDBDatabase = IDBDatabase;
        globalThis.IDBTransaction = IDBTransaction;
        globalThis.IDBObjectStore = IDBObjectStore;
        globalThis.IDBRequest = IDBRequest;
        globalThis.IDBOpenDBRequest = IDBOpenDBRequest;
        globalThis.IDBKeyRange = IDBKeyRange;
        globalThis.IDBCursor = IDBCursor;
        _maskAsNative(IDBFactory, IDBDatabase, IDBTransaction, IDBObjectStore, IDBRequest, IDBOpenDBRequest, IDBKeyRange, IDBCursor);
    }

    // ================================================================
    // Shared Web APIs Part 3.
    // ================================================================
    if (!globalThis.FileReader) {
        // Real FileReader. Previously a no-op stub that returned empty
        // strings/buffers — some challenge scripts call readAsDataURL(blob)
        // to base64-encode a payload before POSTing it; an empty result
        // caused the script to bail with "challenge data URL was
        // malformed", and the server then served a small stub page.
        const _readerEncode = (bytes) => {
            // Manual base64 over Uint8Array via btoa(binary-string). btoa
            // is fine on UTF-8-clean ranges (0-255); we feed it raw bytes
            // mapped through String.fromCharCode.
            let bin = '';
            // Chunk to avoid blowing the call stack on large blobs.
            const CHUNK = 0x8000;
            for (let i = 0; i < bytes.length; i += CHUNK) {
                bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
            }
            return btoa(bin);
        };
        const _readerDispatch = (self, name) => {
            const ev = { target: self, type: name };
            if (self['on' + name]) setTimeout(() => self['on' + name](ev), 0);
        };
        globalThis.FileReader = class FileReader extends EventTarget {
            static EMPTY = 0; static LOADING = 1; static DONE = 2;
            constructor() {
                super();
                this.readyState = 0; this.result = null; this.error = null;
                this.onload = null; this.onloadstart = null; this.onloadend = null;
                this.onprogress = null; this.onerror = null; this.onabort = null;
            }
            readAsText(blob, encoding) {
                try {
                    const bytes = (blob && blob._data) ? blob._data : new Uint8Array(0);
                    const dec = new TextDecoder(encoding || 'utf-8');
                    this.result = dec.decode(bytes);
                } catch (e) { this.error = e; this.result = null; }
                this.readyState = 2;
                _readerDispatch(this, 'load'); _readerDispatch(this, 'loadend');
            }
            readAsDataURL(blob) {
                try {
                    const bytes = (blob && blob._data) ? blob._data : new Uint8Array(0);
                    const b64 = _readerEncode(bytes);
                    const mime = (blob && blob.type) || 'application/octet-stream';
                    this.result = `data:${mime};base64,${b64}`;
                } catch (e) { this.error = e; this.result = null; }
                this.readyState = 2;
                _readerDispatch(this, 'load'); _readerDispatch(this, 'loadend');
            }
            readAsArrayBuffer(blob) {
                try {
                    const bytes = (blob && blob._data) ? blob._data : new Uint8Array(0);
                    // Copy into a fresh ArrayBuffer matching the blob exactly
                    const buf = new ArrayBuffer(bytes.byteLength);
                    new Uint8Array(buf).set(bytes);
                    this.result = buf;
                } catch (e) { this.error = e; this.result = null; }
                this.readyState = 2;
                _readerDispatch(this, 'load'); _readerDispatch(this, 'loadend');
            }
            readAsBinaryString(blob) {
                try {
                    const bytes = (blob && blob._data) ? blob._data : new Uint8Array(0);
                    let bin = '';
                    const CHUNK = 0x8000;
                    for (let i = 0; i < bytes.length; i += CHUNK) {
                        bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
                    }
                    this.result = bin;
                } catch (e) { this.error = e; this.result = null; }
                this.readyState = 2;
                _readerDispatch(this, 'load'); _readerDispatch(this, 'loadend');
            }
            abort() { this.readyState = 2; _readerDispatch(this, 'abort'); _readerDispatch(this, 'loadend'); }
        };
        _maskAsNative(globalThis.FileReader);
    }
    if (!globalThis.ImageBitmap) {
        globalThis.ImageBitmap = class ImageBitmap { constructor() { this.width = 0; this.height = 0; } close() {} };
        _maskAsNative(globalThis.ImageBitmap);
    }
    if (!globalThis.createImageBitmap) {
        globalThis.createImageBitmap = function() { return Promise.resolve(new globalThis.ImageBitmap()); };
        _maskAsNative(globalThis.createImageBitmap);
    }
    if (!globalThis.DOMPoint) {
        globalThis.DOMPoint = class DOMPoint {
            constructor(x, y, z, w) { this.x = x || 0; this.y = y || 0; this.z = z || 0; this.w = w === undefined ? 1 : w; }
            matrixTransform() { return new DOMPoint(this.x, this.y, this.z, this.w); }
            toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
            static fromPoint(p) { return new DOMPoint(p?.x, p?.y, p?.z, p?.w); }
        };
        globalThis.DOMPointReadOnly = globalThis.DOMPoint;
        _maskAsNative(globalThis.DOMPoint);
    }
    if (!globalThis.DOMMatrix) {
        globalThis.DOMMatrix = class DOMMatrix {
            constructor(init) {
                this.a = 1; this.b = 0; this.c = 0; this.d = 1; this.e = 0; this.f = 0;
                this.m11 = 1; this.m12 = 0; this.m13 = 0; this.m14 = 0;
                this.m21 = 0; this.m22 = 1; this.m23 = 0; this.m24 = 0;
                this.m31 = 0; this.m32 = 0; this.m33 = 1; this.m34 = 0;
                this.m41 = 0; this.m42 = 0; this.m43 = 0; this.m44 = 1;
                this.is2D = true; this.isIdentity = true;
            }
            multiply() { return new DOMMatrix(); } translate() { return new DOMMatrix(); } scale() { return new DOMMatrix(); } rotate() { return new DOMMatrix(); } inverse() { return new DOMMatrix(); } transformPoint(p) { return new DOMPoint(p?.x, p?.y, p?.z, p?.w); } toString() { return "matrix(1, 0, 0, 1, 0, 0)"; } toFloat32Array() { return new Float32Array(16); } toFloat64Array() { return new Float64Array(16); } static fromMatrix(m) { return new DOMMatrix(); } static fromFloat32Array() { return new DOMMatrix(); } static fromFloat64Array() { return new DOMMatrix(); }
        };
        globalThis.DOMMatrixReadOnly = globalThis.DOMMatrix;
        globalThis.WebKitCSSMatrix = globalThis.DOMMatrix;
        _maskAsNative(globalThis.DOMMatrix);
    }

})(globalThis);
