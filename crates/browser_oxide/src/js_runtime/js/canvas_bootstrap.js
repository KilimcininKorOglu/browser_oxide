((globalThis) => {
    const ops = Deno.core.ops;

    // -- Canvas-based font detection support -----------------------------
    // Some scripts detect installed fonts
    // by comparing measureText widths across candidate families: if
    // measureText("...", "Arial") differs from measureText("...", "sans-serif")
    // the family is reported as installed. Our font_database.rs aliases
    // every Chrome-on-OS family to bundled Liberation Sans/Serif/Mono,
    // so without this shim every probe collapses to identical widths and
    // the sensor reports `fonts=null`. Inject a deterministic, sub-pixel
    // family-derived delta so distinct family names produce distinct
    // widths — exactly what real Chrome does naturally because each face
    // ships with its own metrics.
    const _fontProbeFnvHash = (str) => {
        let h = 2166136261 >>> 0;
        for (let i = 0; i < str.length; i++) {
            h ^= str.charCodeAt(i);
            h = (h + ((h << 1) + (h << 4) + (h << 7) + (h << 8) + (h << 24))) >>> 0;
        }
        return h;
    };
    // Mirror the fonts present on Chrome for each OS — keep in sync with
    // `window_bootstrap.js` `Font enumeration spoofing` block.
    const _FONT_LIST_BY_OS = {
        "Windows": new Set([
            "arial","arial black","calibri","cambria","comic sans ms","consolas",
            "courier new","georgia","impact","lucida console","segoe ui","tahoma",
            "times new roman","trebuchet ms","verdana",
        ]),
        "macOS": new Set([
            "arial","arial black","courier new","georgia","helvetica",
            "helvetica neue","lucida grande","menlo","monaco","sf pro",
            "times new roman","trebuchet ms","verdana",
        ]),
        "Linux": new Set([
            "arial","courier new","dejavu sans","dejavu sans mono","dejavu serif",
            "liberation mono","liberation sans","liberation serif","noto sans",
            "times new roman","ubuntu","verdana",
        ]),
    };
    const _resolveInstalledFonts = () => {
        const os = _getOsName();
        return _FONT_LIST_BY_OS[os] || _FONT_LIST_BY_OS["Linux"];
    };
    const _getOsName = () => {
        try {
            const has = ops.op_has_stealth_profile && ops.op_has_stealth_profile();
            return has ? (ops.op_get_profile_value("os_name") || "Linux") : "Linux";
        } catch (_e) {
            return "Linux";
        }
    };
    let _canvasSeedCache = null;
    const _getCanvasSeed = () => {
        if (_canvasSeedCache !== null) return _canvasSeedCache;
        try {
            const has = ops.op_has_stealth_profile && ops.op_has_stealth_profile();
            const raw = has ? ops.op_get_profile_value("canvas_seed") : "0";
            _canvasSeedCache = BigInt(raw || "0");
        } catch (_e) {
            _canvasSeedCache = 0n;
        }
        return _canvasSeedCache;
    };
    const _GENERIC_FAMILIES = new Set(["sans-serif","serif","monospace","cursive","fantasy","system-ui","ui-sans-serif","ui-serif","ui-monospace"]);
    // 0.0 .. ~3.5 px deterministic delta. Sub-character-width so layout
    // stays stable, large enough to clear 1e-3 fingerprint comparisons.

    // Parse CSS color to [r, g, b, a]
    function _parseColor(str) {
        const named = { red:[255,0,0,255], green:[0,128,0,255], blue:[0,0,255,255],
            black:[0,0,0,255], white:[255,255,255,255], yellow:[255,255,0,255],
            cyan:[0,255,255,255], magenta:[255,0,255,255], transparent:[0,0,0,0] };
        if (named[str]) return named[str];
        if (str.startsWith('#')) {
            const h = str.slice(1);
            if (h.length === 3) return [parseInt(h[0]+h[0],16), parseInt(h[1]+h[1],16), parseInt(h[2]+h[2],16), 255];
            if (h.length === 6) return [parseInt(h.slice(0,2),16), parseInt(h.slice(2,4),16), parseInt(h.slice(4,6),16), 255];
        }
        const m = str.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)/);
        if (m) return [+m[1], +m[2], +m[3], m[4] !== undefined ? Math.round(+m[4]*255) : 255];
        return [0, 0, 0, 255];
    }

    class ImageData {
        constructor(data, width, height) {
            if (arguments.length === 2) {
                // constructor(width, height)
                height = width;
                width = data;
                data = new Uint8ClampedArray(width * height * 4);
            }
            this.data = data;
            this.width = width;
            this.height = height;
        }
    }
    globalThis.ImageData = ImageData;
    _maskFunction(ImageData, 'ImageData');
    Object.defineProperty(ImageData.prototype, Symbol.toStringTag, { value: "ImageData", configurable: true });

    // The element or OffscreenCanvas each 2D context belongs to. Chrome
    // exposes it through a prototype getter, never an own property.
    const _ctxOwner = new WeakMap();
    // The global mask helper is removed after bootstrap; keep a reference
    // for getters built lazily at runtime.
    const _maskGetter = globalThis._maskFunction;

    // TextMetrics values live in a WeakMap behind enumerable prototype
    // getters, the shape WebIDL attributes have in Chrome.
    const _metrics = new WeakMap();
    const _metricKeys = ["width", "actualBoundingBoxLeft", "actualBoundingBoxRight",
        "fontBoundingBoxAscent", "fontBoundingBoxDescent", "actualBoundingBoxAscent",
        "actualBoundingBoxDescent", "emHeightAscent", "emHeightDescent",
        "hangingBaseline", "alphabeticBaseline", "ideographicBaseline"];
    let _metricsProto = null;
    const _textMetricsProto = () => {
        if (_metricsProto) return _metricsProto;
        const C = globalThis.TextMetrics;
        _metricsProto = (C && C.prototype) || {};
        for (const key of _metricKeys) {
            if (Object.getOwnPropertyDescriptor(_metricsProto, key)) continue;
            const get = { [key]() { return _metrics.get(this)[key]; } }[key];
            _maskGetter(get, "get " + key);
            Object.defineProperty(_metricsProto, key, { get, enumerable: true, configurable: true });
        }
        return _metricsProto;
    };
    const _makeTextMetrics = (values) => {
        const m = Object.create(_textMetricsProto());
        _metrics.set(m, values);
        return m;
    };

    class CanvasRenderingContext2D {
        #id;
        constructor(id) { this.#id = id; }
        get canvas() { return _ctxOwner.get(this) ?? null; }

        // Style
        set fillStyle(v) {
            if (v && typeof v === "object" && v._type) {
                // Gradient object
                const stops = (v._stops || []).map(s => {
                    const c = _parseColor(s.color);
                    return [s.offset, c[0], c[1], c[2], c[3]];
                });
                let coords;
                if (v._type === "linear") {
                    coords = [v._x0, v._y0, v._x1, v._y1];
                } else {
                    coords = [v._x0, v._y0, v._r0, v._x1, v._y1, v._r1];
                }
                ops.op_canvas_set_fill_gradient(this.#id, v._type, JSON.stringify({ coords, stops }));
            } else {
                ops.op_canvas_set_fill_style(this.#id, String(v));
            }
        }
        set strokeStyle(v) { ops.op_canvas_set_stroke_style(this.#id, String(v)); }
        set lineWidth(v) { ops.op_canvas_set_line_width(this.#id, +v); }
        set globalAlpha(v) { ops.op_canvas_set_global_alpha(this.#id, +v); }
        set font(v) { this._font = String(v); ops.op_canvas_set_font(this.#id, this._font); }
        get font() {
            const raw = this._font || "10px sans-serif";
            // Chrome normalizes the shorthand on read: family names that
            // need quoting come back double-quoted ("40px \"Andale Mono\""),
            // never with the author's single quotes.
            const m = /^(.*?\d+(?:\.\d+)?(?:px|pt|em|%)(?:\s*\/\s*[^(,)]+)?)\s+(.*)$/.exec(raw.trim());
            if (!m) return raw;
            const families = m[2].split(",").map(f => {
                let t = f.trim();
                if ((t.startsWith(") && t.endsWith(")) || (t.startsWith('"') && t.endsWith('"'))) {
                    t = t.slice(1, -1);
                }
                return /^[A-Za-z-]+(?:\s+[A-Za-z-]+)*$/.test(t) && !t.includes(" ")
                    ? t : '"' + t.replace(/"/g, '') + '"';
            });
            return m[1] + " " + families.join(", ");
        }

        // Rectangles
        fillRect(x, y, w, h) { ops.op_canvas_fill_rect(this.#id, x, y, w, h); }
        strokeRect(x, y, w, h) { ops.op_canvas_stroke_rect(this.#id, x, y, w, h); }
        clearRect(x, y, w, h) { ops.op_canvas_clear_rect(this.#id, x, y, w, h); }

        // Path
        beginPath() { ops.op_canvas_begin_path(this.#id); }
        moveTo(x, y) { ops.op_canvas_move_to(this.#id, x, y); }
        lineTo(x, y) { ops.op_canvas_line_to(this.#id, x, y); }
        fill() { ops.op_canvas_fill(this.#id); }
        stroke() { ops.op_canvas_stroke(this.#id); }
        closePath() { ops.op_canvas_close_path(this.#id); }
        arc(x, y, r, startAngle, endAngle, counterclockwise) {
            ops.op_canvas_arc(this.#id, x, y, r, startAngle, endAngle, !!counterclockwise);
        }
        arcTo(x1, y1, x2, y2, r) {
            ops.op_canvas_arc_to(this.#id, x1, y1, x2, y2, r);
        }
        bezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y) {
            ops.op_canvas_bezier_curve_to(this.#id, cp1x, cp1y, cp2x, cp2y, x, y);
        }
        quadraticCurveTo(cpx, cpy, x, y) {
            ops.op_canvas_quadratic_curve_to(this.#id, cpx, cpy, x, y);
        }
        ellipse(x, y, rx, ry, rotation, startAngle, endAngle, counterclockwise) {
            ops.op_canvas_ellipse(this.#id, x, y, rx, ry, rotation, startAngle, endAngle, !!counterclockwise);
        }
        rect(x, y, w, h) { this.moveTo(x,y); this.lineTo(x+w,y); this.lineTo(x+w,y+h); this.lineTo(x,y+h); this.closePath(); }

        // Text
        fillText(text, x, y) { ops.op_canvas_fill_text(this.#id, text, x, y); }
        strokeText(text, x, y) { ops.op_canvas_stroke_text(this.#id, text, x, y); }
        measureText(text) {
            // Full 13-field TextMetrics shaped in Rust. NO synthetic
            // per-family delta: the resolved faces already differentiate
            // (real system fonts on macOS, metric-compatible Liberation on
            // Linux), and real Chrome adds nothing here — the old
            // _fontFamilyWidthDelta hack shifted every width by up to
            // +0.87px per char against the real browser's values.
            const m = ops.op_canvas_measure_text_full(this.#id, text);
            return _makeTextMetrics({
                width: m.width,
                actualBoundingBoxLeft: m.actual_bounding_box_left,
                actualBoundingBoxRight: m.actual_bounding_box_right,
                actualBoundingBoxAscent: m.actual_bounding_box_ascent,
                actualBoundingBoxDescent: m.actual_bounding_box_descent,
                fontBoundingBoxAscent: m.font_bounding_box_ascent,
                fontBoundingBoxDescent: m.font_bounding_box_descent,
                emHeightAscent: m.em_height_ascent,
                emHeightDescent: m.em_height_descent,
                alphabeticBaseline: m.alphabetic_baseline,
                hangingBaseline: m.hanging_baseline,
                ideographicBaseline: m.ideographic_baseline,
            });
        }

        // Transform
        save() { ops.op_canvas_save(this.#id); }
        restore() { ops.op_canvas_restore(this.#id); }
        translate(x, y) { ops.op_canvas_translate(this.#id, x, y); }
        rotate(angle) { ops.op_canvas_rotate(this.#id, angle); }
        scale(x, y) { ops.op_canvas_scale(this.#id, x, y); }
        setTransform(a, b, c, d, e, f) {
            // Spec also accepts a single DOMMatrix-init dict; handle both shapes.
            if (typeof a === "object" && a !== null) {
                ops.op_canvas_set_transform(
                    this.#id, a.a ?? 1, a.b ?? 0, a.c ?? 0, a.d ?? 1, a.e ?? 0, a.f ?? 0
                );
            } else {
                ops.op_canvas_set_transform(this.#id, a, b, c, d, e, f);
            }
        }
        resetTransform() { ops.op_canvas_reset_transform(this.#id); }
        getTransform() { return {a:1,b:0,c:0,d:1,e:0,f:0}; }

        // Image data — real pixel ops
        getImageData(x, y, w, h) {
            const raw = ops.op_canvas_get_image_data(this.#id, x, y, w, h);
            return new ImageData(new Uint8ClampedArray(raw), w, h);
        }
        putImageData(imageData, dx, dy) {
            ops.op_canvas_put_image_data(this.#id, imageData.data, dx, dy, imageData.width, imageData.height);
        }
        createImageData(w, h) { return new ImageData(w, h); }
        drawImage(source, dx, dy) {
            // source can be another canvas element — get its internal ID
            if (source && source._canvasId !== undefined) {
                ops.op_canvas_draw_image(this.#id, source._canvasId, dx || 0, dy || 0);
            }
        }

        // Gradient — JS-side objects that track color stops
        createLinearGradient(x0, y0, x1, y1) {
            const stops = [];
            return {
                addColorStop(offset, color) { stops.push({ offset, color }); },
                _stops: stops, _type: 'linear', _x0: x0, _y0: y0, _x1: x1, _y1: y1,
            };
        }
        createRadialGradient(x0, y0, r0, x1, y1, r1) {
            const stops = [];
            return {
                addColorStop(offset, color) { stops.push({ offset, color }); },
                _stops: stops, _type: 'radial', _x0: x0, _y0: y0, _r0: r0, _x1: x1, _y1: y1, _r1: r1,
            };
        }
        createPattern(image, repetition) { return { _image: image, _repetition: repetition || 'repeat' }; }

        // Clip
        clip() {}
        isPointInPath() { return false; }
        isPointInStroke() { return false; }
    }

    const _glInit = (ctx, canvasId, width, height) => {
        ctx._canvasId = canvasId;
        ctx._width = width || 300;
        ctx._height = height || 150;
        ctx._clearColor = [0, 0, 0, 0];
        ctx.canvas = null;
        ctx.drawingBufferWidth = ctx._width;
        ctx.drawingBufferHeight = ctx._height;
    };

    // The errors a context has recorded and getError() has not yet
    // reported: each distinct code once, oldest first, as GL error flags.
    const _glErrors = new WeakMap();
    const _glRecordError = (ctx, code) => {
        const pending = _glErrors.get(ctx) || [];
        if (!pending.includes(code)) pending.push(code);
        _glErrors.set(ctx, pending);
    };

    // WebGL — routes through Canvas2D backend for real pixel output.
    // Some scripts call readPixels() after clearColor()+clear() and expect real data.
    // The IDL constants are installed from _GL_CONSTS below.
    class WebGLRenderingContext {
        constructor(canvasId, width, height) {
            _glInit(this, canvasId, width, height);
        }

        // --- Real operations via Canvas2D backend ---
        clearColor(r, g, b, a) {
            this._clearColor = [Math.round(r*255), Math.round(g*255), Math.round(b*255), a];
        }
        clear(mask) {
            if (mask & 0x4000 && this._canvasId !== undefined) { // COLOR_BUFFER_BIT
                const [r, g, b, a] = this._clearColor;
                const color = `rgba(${r},${g},${b},${a})`;
                ops.op_canvas_set_fill_style(this._canvasId, color);
                ops.op_canvas_fill_rect(this._canvasId, 0, 0, this._width, this._height);
            }
        }
        readPixels(x, y, w, h, format, type, pixels) {
            if (this._canvasId === undefined || !pixels) return;
            // Canvas2D stores pixels top-down, WebGL is bottom-up — flip Y
            const flippedY = this._height - y - h;
            const data = ops.op_canvas_get_image_data(this._canvasId, x, Math.max(0, flippedY), w, h);
            for (let i = 0; i < data.length && i < pixels.length; i++) {
                pixels[i] = data[i];
            }
        }
        viewport(x, y, w, h) {
            this._width = w || this._width;
            this._height = h || this._height;
        }

        // --- Parameter queries (fingerprint-relevant values) ---
        //
        // All values come from the active StealthProfile's gpu_profile entry.
        // Loaded lazily the first time getParameter is called and cached on
        // the WebGLRenderingContext constructor itself (shared across
        // instances). Implementation note: this is now a STATIC accessor
        // wrapper around a closure-scoped cache loader so the methods
        // below don't reference `this._g` — that meant
        // `getParameter.call(somethingElse)` threw
        // `TypeError: this._g is not a function`, which some scripts
        // detect. Real Chrome's native methods don't have that dependency.
        static _g() {
            if (WebGLRenderingContext._gpuCache) return WebGLRenderingContext._gpuCache;
            // Defaults — used when no stealth profile is active. Must match
            // stealth::gpu::common_params_desktop() so probes that check for
            // non-zero MAX_TEXTURE_SIZE etc. don't see `null` in headless mode.
            // Defaults match captured Chrome 147 on macOS arm64
            // (tests/fixtures/chrome147/captured_macos_arm64.json).
            let vendor = "WebKit";
            let renderer = "WebKit WebGL";
            let version = "WebGL 2.0 (OpenGL ES 3.0 Chromium)";
            let shadingLang = "WebGL GLSL ES 3.00 (OpenGL ES GLSL ES 3.0 Chromium)";
            let unmaskedVendor = "Google Inc. (Apple)";
            let unmaskedRenderer = "ANGLE (Apple, ANGLE Metal Renderer: Apple M3, Unspecified Version)";
            let extensions = [];
            let params = {
                0x0D33: 16384,         // MAX_TEXTURE_SIZE
                0x851C: 16384,         // MAX_CUBE_MAP_TEXTURE_SIZE
                0x84E8: 16384,         // MAX_RENDERBUFFER_SIZE
                0x8073: 2048,          // MAX_3D_TEXTURE_SIZE
                0x8869: 16,            // MAX_VERTEX_ATTRIBS
                0x8DFB: 1024,          // MAX_VERTEX_UNIFORM_VECTORS
                0x8DFD: 15,            // MAX_VARYING_VECTORS
                0x8DFC: 1024,          // MAX_FRAGMENT_UNIFORM_VECTORS
                0x8872: 16,            // MAX_TEXTURE_IMAGE_UNITS
                0x8B4D: 16,            // MAX_VERTEX_TEXTURE_IMAGE_UNITS
                0x8B4C: 32,            // MAX_COMBINED_TEXTURE_IMAGE_UNITS
                // ALIASED_POINT_SIZE_RANGE — captured Chrome 147 macOS: [1, 511] typical
                0x846D: [1.0, 511.0],
                0x846E: [1.0, 1.0],    // ALIASED_LINE_WIDTH_RANGE — Chrome ANGLE on every OS = [1,1]
                0x0D3A: [16384, 16384],// MAX_VIEWPORT_DIMS — captured Chrome 147 macOS
                0x0D56: 8,             // DEPTH_BITS
                0x0D57: 8,             // STENCIL_BITS
                0x80AA: 2,             // SAMPLE_BUFFERS
                0x80A9: 4,             // SAMPLES
                0x8D57: 4,             // MAX_SAMPLES — ANGLE Metal on Apple silicon
            };
            let shaderPrec = {};
            try {
                if (ops.op_has_stealth_profile()) {
                    const s = (k) => ops.op_get_profile_value(k);
                    unmaskedVendor = s("webgl_unmasked_vendor") || unmaskedVendor;
                    unmaskedRenderer = s("webgl_unmasked_renderer") || unmaskedRenderer;
                    version = s("webgl_version") || version;
                    shadingLang = s("webgl_shading_language_version") || shadingLang;
                    const extsJson = s("webgl_extensions");
                    if (extsJson) {
                        try { extensions = JSON.parse(extsJson); } catch {}
                    }
                    const paramsJson = s("webgl_params");
                    if (paramsJson) {
                        try {
                            const arr = JSON.parse(paramsJson);
                            // Array of [glenum, value] pairs → keyed object
                            for (const [k, v] of arr) params[k] = v;
                        } catch {}
                    }
                    const spJson = s("webgl_shader_precision");
                    if (spJson) {
                        try {
                            // Array of [shader_type, precision_type, [min, max, precision]]
                            const arr = JSON.parse(spJson);
                            for (const [st, pt, v] of arr) {
                                shaderPrec[`${st}:${pt}`] = { rangeMin: v[0], rangeMax: v[1], precision: v[2] };
                            }
                        } catch {}
                    }
                }
            } catch {}
            // Firefox WebGL coherence: Gecko reports "Mozilla" for VENDOR /
            // RENDERER and the UNMASKED_*_WEBGL strings, and a VERSION without
            // the "(OpenGL ES … Chromium)" suffix. Chrome's GL identity
            // ("WebKit" / "Google Inc." / "ANGLE (…)") under a Firefox UA is a
            // 100% tell, so override the whole GL identity for the FF profile.
            try {
                if (ops.op_has_stealth_profile() &&
                    /Firefox\//.test(ops.op_get_profile_value("user_agent") || "")) {
                    vendor = "Mozilla";
                    renderer = "Mozilla";
                    unmaskedVendor = "Mozilla";
                    unmaskedRenderer = "Mozilla";
                    version = "WebGL 2.0";
                    shadingLang = "WebGL GLSL ES 3.00";
                }
            } catch {}
            WebGLRenderingContext._gpuCache = {
                vendor, renderer, version, shadingLang,
                unmaskedVendor, unmaskedRenderer,
                extensions, params, shaderPrec,
            };
            return WebGLRenderingContext._gpuCache;
        }
        // FIX-D2: the WebGL **1.0** surface. `_g()` above is the WebGL **2.0**
        // surface; a `getContext("webgl")` context must NOT report the WebGL 2
        // version string or expose WebGL-2-only extensions (e.g.
        // `EXT_color_buffer_float`) — that cross-API mismatch differs from
        // real Chrome. Derived from the active
        // profile's `webgl1_*` values; falls back to `_g()` when the profile has
        // no distinct WebGL 1 surface (legacy profiles) → no behaviour change.
        static _g1() {
            if (WebGLRenderingContext._gpuCache1) return WebGLRenderingContext._gpuCache1;
            const base = WebGLRenderingContext._g();
            // Start from the base surface, then downgrade the version strings to
            // WebGL 1 whenever base describes a WebGL 2 surface (the apple_m3
            // default + the no-profile fallback). Legacy profiles whose shared
            // field already holds WebGL 1 data (e.g. nvidia) or masked Firefox
            // keep their base strings. Extensions: an empty list defers to
            // getSupportedExtensions()'s own WebGL-1 fallback.
            let version = base.version;
            let shadingLang = base.shadingLang;
            let extensions = base.extensions;
            const _ffWebGL = (function () {
                try { return ops.op_has_stealth_profile() && /Firefox\//.test(ops.op_get_profile_value("user_agent") || ""); }
                catch { return false; }
            })();
            if (/^WebGL 2/.test(version)) {
                // Firefox WebGL 1 reports "WebGL 1.0" with no "(OpenGL ES … Chromium)" suffix.
                version = _ffWebGL ? "WebGL 1.0" : "WebGL 1.0 (OpenGL ES 2.0 Chromium)";
                shadingLang = _ffWebGL ? "WebGL GLSL ES 1.0" : "WebGL GLSL ES 1.0 (OpenGL ES GLSL ES 1.0 Chromium)";
            }
            try {
                if (ops.op_has_stealth_profile()) {
                    const v = ops.op_get_profile_value("webgl1_version");
                    const sl = ops.op_get_profile_value("webgl1_shading_language_version");
                    const extJson = ops.op_get_profile_value("webgl1_extensions");
                    if (v) version = v;
                    if (sl) shadingLang = sl;
                    if (extJson) {
                        try { const e = JSON.parse(extJson); if (e && e.length) extensions = e; } catch {}
                    }
                }
            } catch {}
            WebGLRenderingContext._gpuCache1 = { ...base, version, shadingLang, extensions };
            return WebGLRenderingContext._gpuCache1;
        }
        // Per-instance surface selector. `_isWebGL2 === false` only for a
        // context handed back by `getContext("webgl"/"experimental-webgl")`.
        // Anything else (incl. `getParameter.call(notACtx)`) → WebGL 2 surface,
        // preserving the pre-FIX-D2 default.
        static _surfaceFor(ctx) {
            return (ctx && ctx._isWebGL2 === false)
                ? WebGLRenderingContext._g1()
                : WebGLRenderingContext._g();
        }
        getParameter(pname) {
            const gpu = WebGLRenderingContext._surfaceFor(this);
            // String-valued parameters
            if (pname === 0x1F00) return gpu.vendor;                // VENDOR
            if (pname === 0x1F01) return gpu.renderer;              // RENDERER
            if (pname === 0x1F02) return gpu.version;               // VERSION
            if (pname === 0x8B8C) return gpu.shadingLang;           // SHADING_LANGUAGE_VERSION
            if (pname === 0x9245) return gpu.unmaskedVendor;        // UNMASKED_VENDOR_WEBGL
            if (pname === 0x9246) return gpu.unmaskedRenderer;      // UNMASKED_RENDERER_WEBGL
            // Runtime-dependent values (not from the catalog)
            if (pname === 0x0BA2) return [0, 0, this._width, this._height]; // VIEWPORT
            // Catalog-sourced numeric/array parameters
            if (gpu.params[pname] !== undefined) return gpu.params[pname];
            return null;
        }
        getSupportedExtensions() {
            const gpu = WebGLRenderingContext._surfaceFor(this);
            // Fallback if the catalog is empty (no profile active).
            // Captured from real Chrome 147 on macOS arm64. WebGL 1 contexts get
            // the WebGL-1 list (extensions promoted to core in WebGL 2 reappear;
            // WebGL-2-only ones absent); WebGL 2 contexts get the 36-ext list.
            if (!gpu.extensions.length) {
                if (this && this._isWebGL2 === false) {
                    return [
                        "ANGLE_instanced_arrays","EXT_blend_minmax","EXT_clip_control",
                        "EXT_color_buffer_half_float","EXT_depth_clamp","EXT_disjoint_timer_query",
                        "EXT_float_blend","EXT_frag_depth","EXT_polygon_offset_clamp","EXT_sRGB",
                        "EXT_shader_texture_lod","EXT_texture_compression_bptc",
                        "EXT_texture_compression_rgtc","EXT_texture_filter_anisotropic",
                        "EXT_texture_mirror_clamp_to_edge","KHR_parallel_shader_compile",
                        "OES_element_index_uint","OES_fbo_render_mipmap","OES_standard_derivatives",
                        "OES_texture_float","OES_texture_float_linear","OES_texture_half_float",
                        "OES_texture_half_float_linear","OES_vertex_array_object",
                        "WEBGL_blend_func_extended","WEBGL_color_buffer_float",
                        "WEBGL_compressed_texture_astc","WEBGL_compressed_texture_etc",
                        "WEBGL_compressed_texture_etc1","WEBGL_compressed_texture_pvrtc",
                        "WEBGL_compressed_texture_s3tc","WEBGL_compressed_texture_s3tc_srgb",
                        "WEBGL_debug_renderer_info","WEBGL_debug_shaders","WEBGL_depth_texture",
                        "WEBGL_draw_buffers","WEBGL_lose_context","WEBGL_multi_draw",
                        "WEBGL_polygon_mode",
                    ];
                }
                return [
                    "EXT_clip_control","EXT_color_buffer_float","EXT_color_buffer_half_float",
                    "EXT_conservative_depth","EXT_depth_clamp","EXT_disjoint_timer_query_webgl2",
                    "EXT_float_blend","EXT_polygon_offset_clamp","EXT_render_snorm",
                    "EXT_texture_compression_bptc","EXT_texture_compression_rgtc",
                    "EXT_texture_filter_anisotropic","EXT_texture_mirror_clamp_to_edge",
                    "EXT_texture_norm16","KHR_parallel_shader_compile",
                    "NV_shader_noperspective_interpolation","OES_draw_buffers_indexed",
                    "OES_sample_variables","OES_shader_multisample_interpolation",
                    "OES_texture_float_linear","WEBGL_blend_func_extended",
                    "WEBGL_clip_cull_distance","WEBGL_compressed_texture_astc",
                    "WEBGL_compressed_texture_etc","WEBGL_compressed_texture_etc1",
                    "WEBGL_compressed_texture_pvrtc","WEBGL_compressed_texture_s3tc",
                    "WEBGL_compressed_texture_s3tc_srgb","WEBGL_debug_renderer_info",
                    "WEBGL_debug_shaders","WEBGL_lose_context","WEBGL_multi_draw",
                    "WEBGL_polygon_mode","WEBGL_provoking_vertex",
                    "WEBGL_render_shared_exponent","WEBGL_stencil_texturing",
                ];
            }
            return gpu.extensions.slice();
        }
        // Answers only for a name in this context's own
        // getSupportedExtensions() list, matched case-insensitively as in
        // Chrome, and hands out the same object on every call.
        getExtension(name) {
            const exts = this.getSupportedExtensions();
            if (!exts) return null;
            const wanted = String(name).toLowerCase();
            const found = exts.find((e) => e.toLowerCase() === wanted);
            return found ? _glExtension(this, found) : null;
        }
        // getContextAttributes — returns the WebGLContextAttributes used at
        // creation. Real Chrome returns these specific defaults.
        getContextAttributes() {
            return {
                alpha: true,
                antialias: true,
                depth: true,
                failIfMajorPerformanceCaveat: false,
                powerPreference: "default",
                premultipliedAlpha: true,
                preserveDrawingBuffer: false,
                stencil: false,
                desynchronized: false,
                xrCompatible: false,
            };
        }
        isContextLost() { return _glLost.has(this); }
        getShaderPrecisionFormat(shaderType, precisionType) {
            const gpu = WebGLRenderingContext._g();
            const key = `${shaderType}:${precisionType}`;
            if (gpu.shaderPrec[key]) return gpu.shaderPrec[key];
            // Fallback for unknown combinations — float-style values (our old behavior)
            return { rangeMin: 127, rangeMax: 127, precision: 23 };
        }

        // --- Shader/program stubs (needed for API surface) ---
        // create*() and is*() come from _glStub, which hands out typed
        // WebGL objects.
        // The engine compiles no GLSL: a shader whose source defines main()
        // compiles, and a program links once it holds a compiled vertex and
        // fragment shader. Measured in Chrome 147: an uncompiled shader
        // reports COMPILE_STATUS false, a program linked without shaders
        // reports LINK_STATUS false, and getUniformLocation() on a program
        // that did not link answers null with INVALID_OPERATION.
        shaderSource(shader, source) {
            const rec = _glRecordOf(shader, 'WebGLShader');
            if (rec) { rec.source = String(source); rec.compiled = false; }
        }
        compileShader(shader) {
            const rec = _glRecordOf(shader, 'WebGLShader');
            if (rec) rec.compiled = /\bvoid\s+main\s*\(/.test(rec.source || '');
        }
        getShaderInfoLog() { return ""; }
        getShaderParameter(shader, pname) {
            const rec = _glRecordOf(shader, 'WebGLShader');
            if (!rec) return null;
            if (pname === 0x8B81) return !!rec.compiled; // COMPILE_STATUS
            if (pname === 0x8B4F) return rec.arg;        // SHADER_TYPE
            if (pname === 0x8B80) return false;          // DELETE_STATUS
            return null;
        }
        attachShader(program, shader) {
            const rec = _glRecordOf(program, 'WebGLProgram');
            if (rec && _glRecordOf(shader, 'WebGLShader')) (rec.shaders ||= new Set()).add(shader);
        }
        linkProgram(program) {
            const rec = _glRecordOf(program, 'WebGLProgram');
            if (!rec) return;
            const types = [...(rec.shaders || [])].map((s) => _glObjects.get(s))
                .filter((s) => s.compiled).map((s) => s.arg);
            rec.linked = types.includes(0x8B31) && types.includes(0x8B30);
        }
        getProgramInfoLog() { return ""; }
        getProgramParameter(program, pname) {
            const rec = _glRecordOf(program, 'WebGLProgram');
            if (pname === 0x8B82) return !!(rec && rec.linked);          // LINK_STATUS
            if (pname === 0x8B85) return rec && rec.shaders ? rec.shaders.size : 0; // ATTACHED_SHADERS
            return true;
        }
        useProgram() {}
        getUniformLocation(program, name) {
            const rec = _glRecordOf(program, 'WebGLProgram');
            if (!rec || !rec.linked) {
                _glRecordError(this, 0x0502);
                return null;
            }
            const ident = (/^\w+/.exec(String(name)) || [''])[0];
            if (!ident) return null;
            const declared = new RegExp('\\buniform\\b[^;]*\\b' + ident + '\\b');
            const found = [...rec.shaders].some((s) => declared.test(_glObjects.get(s).source));
            return found ? _glMakeObject('WebGLUniformLocation') : null;
        }
        getAttribLocation() { return 0; }
        uniform1f() {}
        uniform1i() {}
        uniform2f() {}
        uniform3f() {}
        uniform4f() {}
        uniformMatrix4fv() {}
        bindBuffer() {}
        bufferData() {}
        enableVertexAttribArray() {}
        disableVertexAttribArray() {}
        vertexAttribPointer() {}
        drawArrays() {}
        drawElements() {}
        bindTexture() {}
        texImage2D() {}
        texParameteri() {}
        activeTexture() {}
        generateMipmap() {}
        bindFramebuffer() {}
        framebufferTexture2D() {}
        bindRenderbuffer() {}
        renderbufferStorage() {}
        framebufferRenderbuffer() {}
        checkFramebufferStatus() { return 0x8CD5; } // FRAMEBUFFER_COMPLETE
        enable() {}
        disable() {}
        blendFunc() {}
        blendEquation() {}
        depthFunc() {}
        depthMask() {}
        colorMask() {}
        scissor() {}
        pixelStorei() {}
        getError() {
            const pending = _glErrors.get(this);
            return pending && pending.length ? pending.shift() : 0;
        }
        flush() {}
        finish() {}
        deleteShader() {}
        deleteProgram() {}
        deleteBuffer() {}
        deleteTexture() {}
        deleteFramebuffer() {}
        deleteRenderbuffer() {}
    }

    // FIX-D2: WebGL2RenderingContext is a SEPARATE constructor from
    // WebGLRenderingContext (real Chrome: `WebGLRenderingContext !==
    // WebGL2RenderingContext`, and a webgl2 ctx has its own constructor +
    // "[object WebGL2RenderingContext]" tag). In Chrome it does not extend
    // WebGLRenderingContext either: its prototype chain ends at
    // Object.prototype and it carries its own copy of every member.
    // Instances carry `_isWebGL2 = true` (set in getContext) so the surface
    // selector returns the WebGL 2 surface.
    class WebGL2RenderingContext {
        constructor(canvasId, width, height) {
            _glInit(this, canvasId, width, height);
        }
    }

    // Chrome 147's WebGL surface, measured member by member: every IDL
    // constant with its value and every method with its `length`. A `*`
    // marks a member only WebGL2RenderingContext has.
    const _GL_CONSTS = (
        'DEPTH_BUFFER_BIT:256,STENCIL_BUFFER_BIT:1024,COLOR_BUFFER_BIT:16384,POINTS:0,LINES:1,' +
        'LINE_LOOP:2,LINE_STRIP:3,TRIANGLES:4,TRIANGLE_STRIP:5,TRIANGLE_FAN:6,ZERO:0,ONE:1,' +
        'SRC_COLOR:768,ONE_MINUS_SRC_COLOR:769,SRC_ALPHA:770,ONE_MINUS_SRC_ALPHA:771,DST_ALPHA:772,' +
        'ONE_MINUS_DST_ALPHA:773,DST_COLOR:774,ONE_MINUS_DST_COLOR:775,SRC_ALPHA_SATURATE:776,' +
        'FUNC_ADD:32774,BLEND_EQUATION:32777,BLEND_EQUATION_RGB:32777,BLEND_EQUATION_ALPHA:34877,' +
        'FUNC_SUBTRACT:32778,FUNC_REVERSE_SUBTRACT:32779,BLEND_DST_RGB:32968,BLEND_SRC_RGB:32969,' +
        'BLEND_DST_ALPHA:32970,BLEND_SRC_ALPHA:32971,CONSTANT_COLOR:32769,' +
        'ONE_MINUS_CONSTANT_COLOR:32770,CONSTANT_ALPHA:32771,ONE_MINUS_CONSTANT_ALPHA:32772,' +
        'BLEND_COLOR:32773,ARRAY_BUFFER:34962,ELEMENT_ARRAY_BUFFER:34963,' +
        'ARRAY_BUFFER_BINDING:34964,ELEMENT_ARRAY_BUFFER_BINDING:34965,STREAM_DRAW:35040,' +
        'STATIC_DRAW:35044,DYNAMIC_DRAW:35048,BUFFER_SIZE:34660,BUFFER_USAGE:34661,' +
        'CURRENT_VERTEX_ATTRIB:34342,FRONT:1028,BACK:1029,FRONT_AND_BACK:1032,TEXTURE_2D:3553,' +
        'CULL_FACE:2884,BLEND:3042,DITHER:3024,STENCIL_TEST:2960,DEPTH_TEST:2929,SCISSOR_TEST:3089,' +
        'POLYGON_OFFSET_FILL:32823,SAMPLE_ALPHA_TO_COVERAGE:32926,SAMPLE_COVERAGE:32928,NO_ERROR:0,' +
        'INVALID_ENUM:1280,INVALID_VALUE:1281,INVALID_OPERATION:1282,OUT_OF_MEMORY:1285,CW:2304,' +
        'CCW:2305,LINE_WIDTH:2849,ALIASED_POINT_SIZE_RANGE:33901,ALIASED_LINE_WIDTH_RANGE:33902,' +
        'CULL_FACE_MODE:2885,FRONT_FACE:2886,DEPTH_RANGE:2928,DEPTH_WRITEMASK:2930,' +
        'DEPTH_CLEAR_VALUE:2931,DEPTH_FUNC:2932,STENCIL_CLEAR_VALUE:2961,STENCIL_FUNC:2962,' +
        'STENCIL_FAIL:2964,STENCIL_PASS_DEPTH_FAIL:2965,STENCIL_PASS_DEPTH_PASS:2966,' +
        'STENCIL_REF:2967,STENCIL_VALUE_MASK:2963,STENCIL_WRITEMASK:2968,STENCIL_BACK_FUNC:34816,' +
        'STENCIL_BACK_FAIL:34817,STENCIL_BACK_PASS_DEPTH_FAIL:34818,' +
        'STENCIL_BACK_PASS_DEPTH_PASS:34819,STENCIL_BACK_REF:36003,STENCIL_BACK_VALUE_MASK:36004,' +
        'STENCIL_BACK_WRITEMASK:36005,VIEWPORT:2978,SCISSOR_BOX:3088,COLOR_CLEAR_VALUE:3106,' +
        'COLOR_WRITEMASK:3107,UNPACK_ALIGNMENT:3317,PACK_ALIGNMENT:3333,MAX_TEXTURE_SIZE:3379,' +
        'MAX_VIEWPORT_DIMS:3386,SUBPIXEL_BITS:3408,RED_BITS:3410,GREEN_BITS:3411,BLUE_BITS:3412,' +
        'ALPHA_BITS:3413,DEPTH_BITS:3414,STENCIL_BITS:3415,POLYGON_OFFSET_UNITS:10752,' +
        'POLYGON_OFFSET_FACTOR:32824,TEXTURE_BINDING_2D:32873,SAMPLE_BUFFERS:32936,SAMPLES:32937,' +
        'SAMPLE_COVERAGE_VALUE:32938,SAMPLE_COVERAGE_INVERT:32939,COMPRESSED_TEXTURE_FORMATS:34467,' +
        'DONT_CARE:4352,FASTEST:4353,NICEST:4354,GENERATE_MIPMAP_HINT:33170,BYTE:5120,' +
        'UNSIGNED_BYTE:5121,SHORT:5122,UNSIGNED_SHORT:5123,INT:5124,UNSIGNED_INT:5125,FLOAT:5126,' +
        'DEPTH_COMPONENT:6402,ALPHA:6406,RGB:6407,RGBA:6408,LUMINANCE:6409,LUMINANCE_ALPHA:6410,' +
        'UNSIGNED_SHORT_4_4_4_4:32819,UNSIGNED_SHORT_5_5_5_1:32820,UNSIGNED_SHORT_5_6_5:33635,' +
        'FRAGMENT_SHADER:35632,VERTEX_SHADER:35633,MAX_VERTEX_ATTRIBS:34921,' +
        'MAX_VERTEX_UNIFORM_VECTORS:36347,MAX_VARYING_VECTORS:36348,' +
        'MAX_COMBINED_TEXTURE_IMAGE_UNITS:35661,MAX_VERTEX_TEXTURE_IMAGE_UNITS:35660,' +
        'MAX_TEXTURE_IMAGE_UNITS:34930,MAX_FRAGMENT_UNIFORM_VECTORS:36349,SHADER_TYPE:35663,' +
        'DELETE_STATUS:35712,LINK_STATUS:35714,VALIDATE_STATUS:35715,ATTACHED_SHADERS:35717,' +
        'ACTIVE_UNIFORMS:35718,ACTIVE_ATTRIBUTES:35721,SHADING_LANGUAGE_VERSION:35724,' +
        'CURRENT_PROGRAM:35725,NEVER:512,LESS:513,EQUAL:514,LEQUAL:515,GREATER:516,NOTEQUAL:517,' +
        'GEQUAL:518,ALWAYS:519,KEEP:7680,REPLACE:7681,INCR:7682,DECR:7683,INVERT:5386,' +
        'INCR_WRAP:34055,DECR_WRAP:34056,VENDOR:7936,RENDERER:7937,VERSION:7938,NEAREST:9728,' +
        'LINEAR:9729,NEAREST_MIPMAP_NEAREST:9984,LINEAR_MIPMAP_NEAREST:9985,' +
        'NEAREST_MIPMAP_LINEAR:9986,LINEAR_MIPMAP_LINEAR:9987,TEXTURE_MAG_FILTER:10240,' +
        'TEXTURE_MIN_FILTER:10241,TEXTURE_WRAP_S:10242,TEXTURE_WRAP_T:10243,TEXTURE:5890,' +
        'TEXTURE_CUBE_MAP:34067,TEXTURE_BINDING_CUBE_MAP:34068,TEXTURE_CUBE_MAP_POSITIVE_X:34069,' +
        'TEXTURE_CUBE_MAP_NEGATIVE_X:34070,TEXTURE_CUBE_MAP_POSITIVE_Y:34071,' +
        'TEXTURE_CUBE_MAP_NEGATIVE_Y:34072,TEXTURE_CUBE_MAP_POSITIVE_Z:34073,' +
        'TEXTURE_CUBE_MAP_NEGATIVE_Z:34074,MAX_CUBE_MAP_TEXTURE_SIZE:34076,TEXTURE0:33984,' +
        'TEXTURE1:33985,TEXTURE2:33986,TEXTURE3:33987,TEXTURE4:33988,TEXTURE5:33989,TEXTURE6:33990,' +
        'TEXTURE7:33991,TEXTURE8:33992,TEXTURE9:33993,TEXTURE10:33994,TEXTURE11:33995,' +
        'TEXTURE12:33996,TEXTURE13:33997,TEXTURE14:33998,TEXTURE15:33999,TEXTURE16:34000,' +
        'TEXTURE17:34001,TEXTURE18:34002,TEXTURE19:34003,TEXTURE20:34004,TEXTURE21:34005,' +
        'TEXTURE22:34006,TEXTURE23:34007,TEXTURE24:34008,TEXTURE25:34009,TEXTURE26:34010,' +
        'TEXTURE27:34011,TEXTURE28:34012,TEXTURE29:34013,TEXTURE30:34014,TEXTURE31:34015,' +
        'ACTIVE_TEXTURE:34016,REPEAT:10497,CLAMP_TO_EDGE:33071,MIRRORED_REPEAT:33648,' +
        'FLOAT_VEC2:35664,FLOAT_VEC3:35665,FLOAT_VEC4:35666,INT_VEC2:35667,INT_VEC3:35668,' +
        'INT_VEC4:35669,BOOL:35670,BOOL_VEC2:35671,BOOL_VEC3:35672,BOOL_VEC4:35673,' +
        'FLOAT_MAT2:35674,FLOAT_MAT3:35675,FLOAT_MAT4:35676,SAMPLER_2D:35678,SAMPLER_CUBE:35680,' +
        'VERTEX_ATTRIB_ARRAY_ENABLED:34338,VERTEX_ATTRIB_ARRAY_SIZE:34339,' +
        'VERTEX_ATTRIB_ARRAY_STRIDE:34340,VERTEX_ATTRIB_ARRAY_TYPE:34341,' +
        'VERTEX_ATTRIB_ARRAY_NORMALIZED:34922,VERTEX_ATTRIB_ARRAY_POINTER:34373,' +
        'VERTEX_ATTRIB_ARRAY_BUFFER_BINDING:34975,IMPLEMENTATION_COLOR_READ_TYPE:35738,' +
        'IMPLEMENTATION_COLOR_READ_FORMAT:35739,COMPILE_STATUS:35713,LOW_FLOAT:36336,' +
        'MEDIUM_FLOAT:36337,HIGH_FLOAT:36338,LOW_INT:36339,MEDIUM_INT:36340,HIGH_INT:36341,' +
        'FRAMEBUFFER:36160,RENDERBUFFER:36161,RGBA4:32854,RGB5_A1:32855,RGB565:36194,' +
        'DEPTH_COMPONENT16:33189,STENCIL_INDEX8:36168,DEPTH_STENCIL:34041,RENDERBUFFER_WIDTH:36162,' +
        'RENDERBUFFER_HEIGHT:36163,RENDERBUFFER_INTERNAL_FORMAT:36164,RENDERBUFFER_RED_SIZE:36176,' +
        'RENDERBUFFER_GREEN_SIZE:36177,RENDERBUFFER_BLUE_SIZE:36178,RENDERBUFFER_ALPHA_SIZE:36179,' +
        'RENDERBUFFER_DEPTH_SIZE:36180,RENDERBUFFER_STENCIL_SIZE:36181,' +
        'FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE:36048,FRAMEBUFFER_ATTACHMENT_OBJECT_NAME:36049,' +
        'FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL:36050,' +
        'FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE:36051,COLOR_ATTACHMENT0:36064,' +
        'DEPTH_ATTACHMENT:36096,STENCIL_ATTACHMENT:36128,DEPTH_STENCIL_ATTACHMENT:33306,NONE:0,' +
        'FRAMEBUFFER_COMPLETE:36053,FRAMEBUFFER_INCOMPLETE_ATTACHMENT:36054,' +
        'FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT:36055,FRAMEBUFFER_INCOMPLETE_DIMENSIONS:36057,' +
        'FRAMEBUFFER_UNSUPPORTED:36061,FRAMEBUFFER_BINDING:36006,RENDERBUFFER_BINDING:36007,' +
        'MAX_RENDERBUFFER_SIZE:34024,INVALID_FRAMEBUFFER_OPERATION:1286,UNPACK_FLIP_Y_WEBGL:37440,' +
        'UNPACK_PREMULTIPLY_ALPHA_WEBGL:37441,CONTEXT_LOST_WEBGL:37442,' +
        'UNPACK_COLORSPACE_CONVERSION_WEBGL:37443,BROWSER_DEFAULT_WEBGL:37444,*READ_BUFFER:3074,' +
        '*UNPACK_ROW_LENGTH:3314,*UNPACK_SKIP_ROWS:3315,*UNPACK_SKIP_PIXELS:3316,' +
        '*PACK_ROW_LENGTH:3330,*PACK_SKIP_ROWS:3331,*PACK_SKIP_PIXELS:3332,*COLOR:6144,*DEPTH:6145,' +
        '*STENCIL:6146,*RED:6403,RGB8:32849,RGBA8:32856,*RGB10_A2:32857,*TEXTURE_BINDING_3D:32874,' +
        '*UNPACK_SKIP_IMAGES:32877,*UNPACK_IMAGE_HEIGHT:32878,*TEXTURE_3D:32879,' +
        '*TEXTURE_WRAP_R:32882,*MAX_3D_TEXTURE_SIZE:32883,*UNSIGNED_INT_2_10_10_10_REV:33640,' +
        '*MAX_ELEMENTS_VERTICES:33000,*MAX_ELEMENTS_INDICES:33001,*TEXTURE_MIN_LOD:33082,' +
        '*TEXTURE_MAX_LOD:33083,*TEXTURE_BASE_LEVEL:33084,*TEXTURE_MAX_LEVEL:33085,*MIN:32775,' +
        '*MAX:32776,*DEPTH_COMPONENT24:33190,*MAX_TEXTURE_LOD_BIAS:34045,' +
        '*TEXTURE_COMPARE_MODE:34892,*TEXTURE_COMPARE_FUNC:34893,*CURRENT_QUERY:34917,' +
        '*QUERY_RESULT:34918,*QUERY_RESULT_AVAILABLE:34919,*STREAM_READ:35041,*STREAM_COPY:35042,' +
        '*STATIC_READ:35045,*STATIC_COPY:35046,*DYNAMIC_READ:35049,*DYNAMIC_COPY:35050,' +
        '*MAX_DRAW_BUFFERS:34852,*DRAW_BUFFER0:34853,*DRAW_BUFFER1:34854,*DRAW_BUFFER2:34855,' +
        '*DRAW_BUFFER3:34856,*DRAW_BUFFER4:34857,*DRAW_BUFFER5:34858,*DRAW_BUFFER6:34859,' +
        '*DRAW_BUFFER7:34860,*DRAW_BUFFER8:34861,*DRAW_BUFFER9:34862,*DRAW_BUFFER10:34863,' +
        '*DRAW_BUFFER11:34864,*DRAW_BUFFER12:34865,*DRAW_BUFFER13:34866,*DRAW_BUFFER14:34867,' +
        '*DRAW_BUFFER15:34868,*MAX_FRAGMENT_UNIFORM_COMPONENTS:35657,' +
        '*MAX_VERTEX_UNIFORM_COMPONENTS:35658,*SAMPLER_3D:35679,*SAMPLER_2D_SHADOW:35682,' +
        '*FRAGMENT_SHADER_DERIVATIVE_HINT:35723,*PIXEL_PACK_BUFFER:35051,' +
        '*PIXEL_UNPACK_BUFFER:35052,*PIXEL_PACK_BUFFER_BINDING:35053,' +
        '*PIXEL_UNPACK_BUFFER_BINDING:35055,*FLOAT_MAT2x3:35685,*FLOAT_MAT2x4:35686,' +
        '*FLOAT_MAT3x2:35687,*FLOAT_MAT3x4:35688,*FLOAT_MAT4x2:35689,*FLOAT_MAT4x3:35690,' +
        '*SRGB:35904,*SRGB8:35905,*SRGB8_ALPHA8:35907,*COMPARE_REF_TO_TEXTURE:34894,*RGBA32F:34836,' +
        '*RGB32F:34837,*RGBA16F:34842,*RGB16F:34843,*VERTEX_ATTRIB_ARRAY_INTEGER:35069,' +
        '*MAX_ARRAY_TEXTURE_LAYERS:35071,*MIN_PROGRAM_TEXEL_OFFSET:35076,' +
        '*MAX_PROGRAM_TEXEL_OFFSET:35077,*MAX_VARYING_COMPONENTS:35659,*TEXTURE_2D_ARRAY:35866,' +
        '*TEXTURE_BINDING_2D_ARRAY:35869,*R11F_G11F_B10F:35898,*UNSIGNED_INT_10F_11F_11F_REV:35899,' +
        '*RGB9_E5:35901,*UNSIGNED_INT_5_9_9_9_REV:35902,*TRANSFORM_FEEDBACK_BUFFER_MODE:35967,' +
        '*MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS:35968,*TRANSFORM_FEEDBACK_VARYINGS:35971,' +
        '*TRANSFORM_FEEDBACK_BUFFER_START:35972,*TRANSFORM_FEEDBACK_BUFFER_SIZE:35973,' +
        '*TRANSFORM_FEEDBACK_PRIMITIVES_WRITTEN:35976,*RASTERIZER_DISCARD:35977,' +
        '*MAX_TRANSFORM_FEEDBACK_INTERLEAVED_COMPONENTS:35978,' +
        '*MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS:35979,*INTERLEAVED_ATTRIBS:35980,' +
        '*SEPARATE_ATTRIBS:35981,*TRANSFORM_FEEDBACK_BUFFER:35982,' +
        '*TRANSFORM_FEEDBACK_BUFFER_BINDING:35983,*RGBA32UI:36208,*RGB32UI:36209,*RGBA16UI:36214,' +
        '*RGB16UI:36215,*RGBA8UI:36220,*RGB8UI:36221,*RGBA32I:36226,*RGB32I:36227,*RGBA16I:36232,' +
        '*RGB16I:36233,*RGBA8I:36238,*RGB8I:36239,*RED_INTEGER:36244,*RGB_INTEGER:36248,' +
        '*RGBA_INTEGER:36249,*SAMPLER_2D_ARRAY:36289,*SAMPLER_2D_ARRAY_SHADOW:36292,' +
        '*SAMPLER_CUBE_SHADOW:36293,*UNSIGNED_INT_VEC2:36294,*UNSIGNED_INT_VEC3:36295,' +
        '*UNSIGNED_INT_VEC4:36296,*INT_SAMPLER_2D:36298,*INT_SAMPLER_3D:36299,' +
        '*INT_SAMPLER_CUBE:36300,*INT_SAMPLER_2D_ARRAY:36303,*UNSIGNED_INT_SAMPLER_2D:36306,' +
        '*UNSIGNED_INT_SAMPLER_3D:36307,*UNSIGNED_INT_SAMPLER_CUBE:36308,' +
        '*UNSIGNED_INT_SAMPLER_2D_ARRAY:36311,*DEPTH_COMPONENT32F:36012,*DEPTH32F_STENCIL8:36013,' +
        '*FLOAT_32_UNSIGNED_INT_24_8_REV:36269,*FRAMEBUFFER_ATTACHMENT_COLOR_ENCODING:33296,' +
        '*FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE:33297,*FRAMEBUFFER_ATTACHMENT_RED_SIZE:33298,' +
        '*FRAMEBUFFER_ATTACHMENT_GREEN_SIZE:33299,*FRAMEBUFFER_ATTACHMENT_BLUE_SIZE:33300,' +
        '*FRAMEBUFFER_ATTACHMENT_ALPHA_SIZE:33301,*FRAMEBUFFER_ATTACHMENT_DEPTH_SIZE:33302,' +
        '*FRAMEBUFFER_ATTACHMENT_STENCIL_SIZE:33303,*FRAMEBUFFER_DEFAULT:33304,' +
        '*UNSIGNED_INT_24_8:34042,*DEPTH24_STENCIL8:35056,*UNSIGNED_NORMALIZED:35863,' +
        '*DRAW_FRAMEBUFFER_BINDING:36006,*READ_FRAMEBUFFER:36008,*DRAW_FRAMEBUFFER:36009,' +
        '*READ_FRAMEBUFFER_BINDING:36010,*RENDERBUFFER_SAMPLES:36011,' +
        '*FRAMEBUFFER_ATTACHMENT_TEXTURE_LAYER:36052,*MAX_COLOR_ATTACHMENTS:36063,' +
        '*COLOR_ATTACHMENT1:36065,*COLOR_ATTACHMENT2:36066,*COLOR_ATTACHMENT3:36067,' +
        '*COLOR_ATTACHMENT4:36068,*COLOR_ATTACHMENT5:36069,*COLOR_ATTACHMENT6:36070,' +
        '*COLOR_ATTACHMENT7:36071,*COLOR_ATTACHMENT8:36072,*COLOR_ATTACHMENT9:36073,' +
        '*COLOR_ATTACHMENT10:36074,*COLOR_ATTACHMENT11:36075,*COLOR_ATTACHMENT12:36076,' +
        '*COLOR_ATTACHMENT13:36077,*COLOR_ATTACHMENT14:36078,*COLOR_ATTACHMENT15:36079,' +
        '*FRAMEBUFFER_INCOMPLETE_MULTISAMPLE:36182,*MAX_SAMPLES:36183,*HALF_FLOAT:5131,*RG:33319,' +
        '*RG_INTEGER:33320,*R8:33321,*RG8:33323,*R16F:33325,*R32F:33326,*RG16F:33327,*RG32F:33328,' +
        '*R8I:33329,*R8UI:33330,*R16I:33331,*R16UI:33332,*R32I:33333,*R32UI:33334,*RG8I:33335,' +
        '*RG8UI:33336,*RG16I:33337,*RG16UI:33338,*RG32I:33339,*RG32UI:33340,' +
        '*VERTEX_ARRAY_BINDING:34229,*R8_SNORM:36756,*RG8_SNORM:36757,*RGB8_SNORM:36758,' +
        '*RGBA8_SNORM:36759,*SIGNED_NORMALIZED:36764,*COPY_READ_BUFFER:36662,' +
        '*COPY_WRITE_BUFFER:36663,*COPY_READ_BUFFER_BINDING:36662,*COPY_WRITE_BUFFER_BINDING:36663,' +
        '*UNIFORM_BUFFER:35345,*UNIFORM_BUFFER_BINDING:35368,*UNIFORM_BUFFER_START:35369,' +
        '*UNIFORM_BUFFER_SIZE:35370,*MAX_VERTEX_UNIFORM_BLOCKS:35371,' +
        '*MAX_FRAGMENT_UNIFORM_BLOCKS:35373,*MAX_COMBINED_UNIFORM_BLOCKS:35374,' +
        '*MAX_UNIFORM_BUFFER_BINDINGS:35375,*MAX_UNIFORM_BLOCK_SIZE:35376,' +
        '*MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS:35377,' +
        '*MAX_COMBINED_FRAGMENT_UNIFORM_COMPONENTS:35379,*UNIFORM_BUFFER_OFFSET_ALIGNMENT:35380,' +
        '*ACTIVE_UNIFORM_BLOCKS:35382,*UNIFORM_TYPE:35383,*UNIFORM_SIZE:35384,' +
        '*UNIFORM_BLOCK_INDEX:35386,*UNIFORM_OFFSET:35387,*UNIFORM_ARRAY_STRIDE:35388,' +
        '*UNIFORM_MATRIX_STRIDE:35389,*UNIFORM_IS_ROW_MAJOR:35390,*UNIFORM_BLOCK_BINDING:35391,' +
        '*UNIFORM_BLOCK_DATA_SIZE:35392,*UNIFORM_BLOCK_ACTIVE_UNIFORMS:35394,' +
        '*UNIFORM_BLOCK_ACTIVE_UNIFORM_INDICES:35395,' +
        '*UNIFORM_BLOCK_REFERENCED_BY_VERTEX_SHADER:35396,' +
        '*UNIFORM_BLOCK_REFERENCED_BY_FRAGMENT_SHADER:35398,*INVALID_INDEX:4294967295,' +
        '*MAX_VERTEX_OUTPUT_COMPONENTS:37154,*MAX_FRAGMENT_INPUT_COMPONENTS:37157,' +
        '*MAX_SERVER_WAIT_TIMEOUT:37137,*OBJECT_TYPE:37138,*SYNC_CONDITION:37139,' +
        '*SYNC_STATUS:37140,*SYNC_FLAGS:37141,*SYNC_FENCE:37142,*SYNC_GPU_COMMANDS_COMPLETE:37143,' +
        '*UNSIGNALED:37144,*SIGNALED:37145,*ALREADY_SIGNALED:37146,*TIMEOUT_EXPIRED:37147,' +
        '*CONDITION_SATISFIED:37148,*WAIT_FAILED:37149,*SYNC_FLUSH_COMMANDS_BIT:1,' +
        '*VERTEX_ATTRIB_ARRAY_DIVISOR:35070,*ANY_SAMPLES_PASSED:35887,' +
        '*ANY_SAMPLES_PASSED_CONSERVATIVE:36202,*SAMPLER_BINDING:35097,*RGB10_A2UI:36975,' +
        '*INT_2_10_10_10_REV:36255,*TRANSFORM_FEEDBACK:36386,*TRANSFORM_FEEDBACK_PAUSED:36387,' +
        '*TRANSFORM_FEEDBACK_ACTIVE:36388,*TRANSFORM_FEEDBACK_BINDING:36389,' +
        '*TEXTURE_IMMUTABLE_FORMAT:37167,*MAX_ELEMENT_INDEX:36203,*TEXTURE_IMMUTABLE_LEVELS:33503,' +
        '*TIMEOUT_IGNORED:-1,*MAX_CLIENT_WAIT_TIMEOUT_WEBGL:37447'
    );
    const _GL_METHODS = (
        'activeTexture:1,attachShader:2,*beginQuery:2,*beginTransformFeedback:1,' +
        'bindAttribLocation:3,*bindBufferBase:3,*bindBufferRange:5,bindRenderbuffer:2,' +
        '*bindSampler:2,*bindTransformFeedback:2,*bindVertexArray:1,blendColor:4,blendEquation:1,' +
        'blendEquationSeparate:2,blendFunc:2,blendFuncSeparate:4,*blitFramebuffer:10,bufferData:3,' +
        'bufferSubData:3,checkFramebufferStatus:1,*clientWaitSync:3,compileShader:1,' +
        'compressedTexImage2D:7,*compressedTexImage3D:8,compressedTexSubImage2D:8,' +
        '*compressedTexSubImage3D:10,*copyBufferSubData:5,copyTexImage2D:8,copyTexSubImage2D:8,' +
        '*copyTexSubImage3D:9,createBuffer:0,createFramebuffer:0,createProgram:0,*createQuery:0,' +
        'createRenderbuffer:0,*createSampler:0,createShader:1,createTexture:0,' +
        '*createTransformFeedback:0,*createVertexArray:0,cullFace:1,deleteBuffer:1,' +
        'deleteFramebuffer:1,deleteProgram:1,*deleteQuery:1,deleteRenderbuffer:1,*deleteSampler:1,' +
        'deleteShader:1,*deleteSync:1,deleteTexture:1,*deleteTransformFeedback:1,' +
        '*deleteVertexArray:1,depthFunc:1,depthMask:1,depthRange:2,detachShader:2,disable:1,' +
        '*drawArraysInstanced:4,*drawElementsInstanced:5,*drawRangeElements:6,enable:1,*endQuery:1,' +
        '*endTransformFeedback:0,*fenceSync:2,finish:0,flush:0,framebufferRenderbuffer:4,' +
        'framebufferTexture2D:5,*framebufferTextureLayer:5,frontFace:1,generateMipmap:1,' +
        'getActiveAttrib:2,getActiveUniform:2,*getActiveUniformBlockName:2,' +
        '*getActiveUniformBlockParameter:3,*getActiveUniforms:3,getAttachedShaders:1,' +
        'getAttribLocation:2,getBufferParameter:2,*getBufferSubData:3,getContextAttributes:0,' +
        'getError:0,getExtension:1,*getFragDataLocation:2,getFramebufferAttachmentParameter:3,' +
        '*getIndexedParameter:2,*getInternalformatParameter:3,getParameter:1,getProgramInfoLog:1,' +
        'getProgramParameter:2,*getQuery:2,*getQueryParameter:2,getRenderbufferParameter:2,' +
        '*getSamplerParameter:2,getShaderInfoLog:1,getShaderParameter:2,getShaderPrecisionFormat:2,' +
        'getShaderSource:1,getSupportedExtensions:0,*getSyncParameter:2,getTexParameter:2,' +
        '*getTransformFeedbackVarying:2,getUniform:2,*getUniformBlockIndex:2,*getUniformIndices:2,' +
        'getUniformLocation:2,getVertexAttrib:2,getVertexAttribOffset:2,hint:2,' +
        '*invalidateFramebuffer:2,*invalidateSubFramebuffer:6,isBuffer:1,isContextLost:0,' +
        'isEnabled:1,isFramebuffer:1,isProgram:1,*isQuery:1,isRenderbuffer:1,*isSampler:1,' +
        'isShader:1,*isSync:1,isTexture:1,*isTransformFeedback:1,*isVertexArray:1,lineWidth:1,' +
        'linkProgram:1,*pauseTransformFeedback:0,pixelStorei:2,polygonOffset:2,*readBuffer:1,' +
        'readPixels:7,renderbufferStorage:4,*renderbufferStorageMultisample:5,' +
        '*resumeTransformFeedback:0,sampleCoverage:2,*samplerParameterf:3,*samplerParameteri:3,' +
        'shaderSource:2,stencilFunc:3,stencilFuncSeparate:4,stencilMask:1,stencilMaskSeparate:2,' +
        'stencilOp:3,stencilOpSeparate:4,texImage2D:6,*texImage3D:10,texParameterf:3,' +
        'texParameteri:3,*texStorage2D:5,*texStorage3D:6,texSubImage2D:7,*texSubImage3D:11,' +
        '*transformFeedbackVaryings:3,*uniform1ui:2,*uniform2ui:3,*uniform3ui:4,*uniform4ui:5,' +
        '*uniformBlockBinding:3,useProgram:1,validateProgram:1,*vertexAttribDivisor:2,' +
        '*vertexAttribI4i:5,*vertexAttribI4ui:5,*vertexAttribIPointer:5,*waitSync:3,bindBuffer:2,' +
        'bindFramebuffer:2,bindTexture:2,clear:1,*clearBufferfi:4,*clearBufferfv:3,' +
        '*clearBufferiv:3,*clearBufferuiv:3,clearColor:4,clearDepth:1,clearStencil:1,colorMask:4,' +
        'disableVertexAttribArray:1,drawArrays:3,*drawBuffers:1,drawElements:4,' +
        'enableVertexAttribArray:1,scissor:4,uniform1f:2,uniform1fv:2,uniform1i:2,uniform1iv:2,' +
        '*uniform1uiv:2,uniform2f:3,uniform2fv:2,uniform2i:3,uniform2iv:2,*uniform2uiv:2,' +
        'uniform3f:4,uniform3fv:2,uniform3i:4,uniform3iv:2,*uniform3uiv:2,uniform4f:5,uniform4fv:2,' +
        'uniform4i:5,uniform4iv:2,*uniform4uiv:2,uniformMatrix2fv:3,*uniformMatrix2x3fv:3,' +
        '*uniformMatrix2x4fv:3,uniformMatrix3fv:3,*uniformMatrix3x2fv:3,*uniformMatrix3x4fv:3,' +
        'uniformMatrix4fv:3,*uniformMatrix4x2fv:3,*uniformMatrix4x3fv:3,vertexAttrib1f:2,' +
        'vertexAttrib1fv:2,vertexAttrib2f:3,vertexAttrib2fv:2,vertexAttrib3f:4,vertexAttrib3fv:2,' +
        'vertexAttrib4f:5,vertexAttrib4fv:2,*vertexAttribI4iv:2,*vertexAttribI4uiv:2,' +
        'vertexAttribPointer:6,viewport:4,drawingBufferStorage:3,makeXRCompatible:0'
    );
    const _glTable = (table) => table.split(',').map((entry) => {
        const webgl2Only = entry.startsWith('*');
        const [name, num] = (webgl2Only ? entry.slice(1) : entry).split(':');
        return { name, num: Number(num), webgl2Only };
    });

    // The objects create*() hands out, with the interface each belongs to.
    // is*() recognises its own kind; isQuery, isVertexArray and
    // isTransformFeedback answer true only once the object has been bound,
    // as in GL.
    const _GL_OBJECT_KINDS = {
        createBuffer: 'WebGLBuffer', createFramebuffer: 'WebGLFramebuffer',
        createProgram: 'WebGLProgram', createRenderbuffer: 'WebGLRenderbuffer',
        createShader: 'WebGLShader', createTexture: 'WebGLTexture',
        createQuery: 'WebGLQuery', createSampler: 'WebGLSampler',
        createTransformFeedback: 'WebGLTransformFeedback',
        createVertexArray: 'WebGLVertexArrayObject', fenceSync: 'WebGLSync',
    };
    const _GL_BIND_BEFORE_IS = {
        beginQuery: 1, bindVertexArray: 0, bindTransformFeedback: 1,
    };
    const _glObjects = new WeakMap();
    // `arg` keeps the create call's argument, the type of a shader.
    const _glMakeObject = (kind, arg) => {
        const C = globalThis[kind];
        const obj = Object.create(C ? C.prototype : Object.prototype);
        _glObjects.set(obj, { kind, arg, bound: false });
        return obj;
    };
    const _glRecordOf = (obj, kind) => {
        const rec = (obj && typeof obj === 'object') ? _glObjects.get(obj) : undefined;
        return rec && rec.kind === kind ? rec : undefined;
    };
    const _glIsKind = (obj, kind) => {
        const rec = (obj && typeof obj === 'object') ? _glObjects.get(obj) : undefined;
        if (!rec || rec.kind !== kind) return false;
        const needsBind = kind === 'WebGLQuery' || kind === 'WebGLVertexArrayObject'
            || kind === 'WebGLTransformFeedback';
        return needsBind ? rec.bound : true;
    };

    // The stub a method gets when the engine does not model it: create*
    // and fenceSync hand out a typed object, is* checks one, bind* marks
    // one bound, get* has nothing to report, everything else is a no-op.
    const _glStub = (name) => {
        const kind = _GL_OBJECT_KINDS[name];
        if (kind) return { [name](arg) { return _glMakeObject(kind, arg); } }[name];
        if (name in _GL_BIND_BEFORE_IS) {
            const at = _GL_BIND_BEFORE_IS[name];
            return { [name]() {
                const rec = _glObjects.get(arguments[at]);
                if (rec) rec.bound = true;
            } }[name];
        }
        const isKind = /^is(Buffer|Framebuffer|Program|Renderbuffer|Shader|Texture|Query|Sampler|Sync|TransformFeedback|VertexArray)$/.exec(name);
        if (isKind) {
            const k = isKind[1] === 'VertexArray' ? 'WebGLVertexArrayObject' : 'WebGL' + isKind[1];
            return { [name](obj) { return _glIsKind(obj, k); } }[name];
        }
        if (name.startsWith('get')) return { [name]() { return null; } }[name];
        return { [name]() {} }[name];
    };

    // Chrome's attribute shapes: constants are read-only, enumerable and
    // non-configurable on both the prototype and the interface object;
    // methods are writable, enumerable and configurable.
    const _glConst = (value) => ({ value, writable: false, enumerable: true, configurable: false });
    const _glMethod = (value) => ({ value, writable: true, enumerable: true, configurable: true });

    // Chrome 147's extension interfaces: `name=Interface|CONST:value|
    // method()length`. The ones Chrome's software renderer does not offer
    // (ASTC, ETC, ETC1, PVRTC, KHR_parallel_shader_compile,
    // WEBGL_blend_func_extended, WEBGL_provoking_vertex, EXT_texture_norm16,
    // EXT_render_snorm, WEBGL_render_shared_exponent) follow their Khronos
    // specifications.
    const _GL_EXTENSIONS = (
        'ANGLE_instanced_arrays=ANGLEInstancedArrays|VERTEX_ATTRIB_ARRAY_DIVISOR_ANGLE:35070|drawArraysInstancedANGLE()4|drawElementsInstancedANGLE()5|vertexAttribDivisorANGLE()2;' +
        'EXT_blend_minmax=EXTBlendMinMax|MIN_EXT:32775|MAX_EXT:32776;' +
        'EXT_clip_control=EXTClipControl|LOWER_LEFT_EXT:36001|UPPER_LEFT_EXT:36002|NEGATIVE_ONE_TO_ONE_EXT:37726|ZERO_TO_ONE_EXT:37727|CLIP_ORIGIN_EXT:37724|CLIP_DEPTH_MODE_EXT:37725|clipControlEXT()2;' +
        'EXT_color_buffer_float=EXTColorBufferFloat;' +
        'EXT_color_buffer_half_float=EXTColorBufferHalfFloat|RGBA16F_EXT:34842|RGB16F_EXT:34843|FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE_EXT:33297|UNSIGNED_NORMALIZED_EXT:35863;' +
        'EXT_conservative_depth=EXTConservativeDepth;' +
        'EXT_depth_clamp=EXTDepthClamp|DEPTH_CLAMP_EXT:34383;' +
        'EXT_disjoint_timer_query=EXTDisjointTimerQuery|QUERY_COUNTER_BITS_EXT:34916|CURRENT_QUERY_EXT:34917|QUERY_RESULT_EXT:34918|QUERY_RESULT_AVAILABLE_EXT:34919|TIME_ELAPSED_EXT:35007|TIMESTAMP_EXT:36392|GPU_DISJOINT_EXT:36795|beginQueryEXT()2|createQueryEXT()0|deleteQueryEXT()1|endQueryEXT()1|getQueryEXT()2|getQueryObjectEXT()2|isQueryEXT()1|queryCounterEXT()2;' +
        'EXT_disjoint_timer_query_webgl2=EXTDisjointTimerQueryWebGL2|QUERY_COUNTER_BITS_EXT:34916|TIME_ELAPSED_EXT:35007|TIMESTAMP_EXT:36392|GPU_DISJOINT_EXT:36795|queryCounterEXT()2;' +
        'EXT_float_blend=EXTFloatBlend;' +
        'EXT_frag_depth=EXTFragDepth;' +
        'EXT_polygon_offset_clamp=EXTPolygonOffsetClamp|POLYGON_OFFSET_CLAMP_EXT:36379|polygonOffsetClampEXT()3;' +
        'EXT_render_snorm=EXTRenderSnorm;' +
        'EXT_sRGB=EXTsRGB|SRGB_EXT:35904|SRGB_ALPHA_EXT:35906|SRGB8_ALPHA8_EXT:35907|FRAMEBUFFER_ATTACHMENT_COLOR_ENCODING_EXT:33296;' +
        'EXT_shader_texture_lod=EXTShaderTextureLOD;' +
        'EXT_texture_compression_bptc=EXTTextureCompressionBPTC|COMPRESSED_RGBA_BPTC_UNORM_EXT:36492|COMPRESSED_SRGB_ALPHA_BPTC_UNORM_EXT:36493|COMPRESSED_RGB_BPTC_SIGNED_FLOAT_EXT:36494|COMPRESSED_RGB_BPTC_UNSIGNED_FLOAT_EXT:36495;' +
        'EXT_texture_compression_rgtc=EXTTextureCompressionRGTC|COMPRESSED_RED_RGTC1_EXT:36283|COMPRESSED_SIGNED_RED_RGTC1_EXT:36284|COMPRESSED_RED_GREEN_RGTC2_EXT:36285|COMPRESSED_SIGNED_RED_GREEN_RGTC2_EXT:36286;' +
        'EXT_texture_filter_anisotropic=EXTTextureFilterAnisotropic|TEXTURE_MAX_ANISOTROPY_EXT:34046|MAX_TEXTURE_MAX_ANISOTROPY_EXT:34047;' +
        'EXT_texture_mirror_clamp_to_edge=EXTTextureMirrorClampToEdge|MIRROR_CLAMP_TO_EDGE_EXT:34627;' +
        'EXT_texture_norm16=EXTTextureNorm16|R16_EXT:33322|RG16_EXT:33324|RGB16_EXT:32852|RGBA16_EXT:32859|R16_SNORM_EXT:36760|RG16_SNORM_EXT:36761|RGB16_SNORM_EXT:36762|RGBA16_SNORM_EXT:36763;' +
        'KHR_parallel_shader_compile=KHRParallelShaderCompile|COMPLETION_STATUS_KHR:37297;' +
        'NV_shader_noperspective_interpolation=NVShaderNoperspectiveInterpolation;' +
        'OES_draw_buffers_indexed=OESDrawBuffersIndexed|blendEquationSeparateiOES()3|blendEquationiOES()2|blendFuncSeparateiOES()5|blendFunciOES()3|colorMaskiOES()5|disableiOES()2|enableiOES()2;' +
        'OES_element_index_uint=OESElementIndexUint;' +
        'OES_fbo_render_mipmap=OESFboRenderMipmap;' +
        'OES_sample_variables=OESSampleVariables;' +
        'OES_shader_multisample_interpolation=OESShaderMultisampleInterpolation|MIN_FRAGMENT_INTERPOLATION_OFFSET_OES:36443|MAX_FRAGMENT_INTERPOLATION_OFFSET_OES:36444|FRAGMENT_INTERPOLATION_OFFSET_BITS_OES:36445;' +
        'OES_standard_derivatives=OESStandardDerivatives|FRAGMENT_SHADER_DERIVATIVE_HINT_OES:35723;' +
        'OES_texture_float=OESTextureFloat;' +
        'OES_texture_float_linear=OESTextureFloatLinear;' +
        'OES_texture_half_float=OESTextureHalfFloat|HALF_FLOAT_OES:36193;' +
        'OES_texture_half_float_linear=OESTextureHalfFloatLinear;' +
        'OES_vertex_array_object=OESVertexArrayObject|VERTEX_ARRAY_BINDING_OES:34229|bindVertexArrayOES()0|createVertexArrayOES()0|deleteVertexArrayOES()0|isVertexArrayOES()0;' +
        'WEBGL_blend_func_extended=WebGLBlendFuncExtended|SRC1_COLOR_WEBGL:35065|SRC1_ALPHA_WEBGL:34185|ONE_MINUS_SRC1_COLOR_WEBGL:35066|ONE_MINUS_SRC1_ALPHA_WEBGL:35067|MAX_DUAL_SOURCE_DRAW_BUFFERS_WEBGL:35068;' +
        'WEBGL_clip_cull_distance=WebGLClipCullDistance|MAX_CLIP_DISTANCES_WEBGL:3378|MAX_CULL_DISTANCES_WEBGL:33529|MAX_COMBINED_CLIP_AND_CULL_DISTANCES_WEBGL:33530|CLIP_DISTANCE0_WEBGL:12288|CLIP_DISTANCE1_WEBGL:12289|CLIP_DISTANCE2_WEBGL:12290|CLIP_DISTANCE3_WEBGL:12291|CLIP_DISTANCE4_WEBGL:12292|CLIP_DISTANCE5_WEBGL:12293|CLIP_DISTANCE6_WEBGL:12294|CLIP_DISTANCE7_WEBGL:12295;' +
        'WEBGL_color_buffer_float=WebGLColorBufferFloat|RGBA32F_EXT:34836|FRAMEBUFFER_ATTACHMENT_COMPONENT_TYPE_EXT:33297|UNSIGNED_NORMALIZED_EXT:35863;' +
        'WEBGL_compressed_texture_astc=WebGLCompressedTextureASTC|COMPRESSED_RGBA_ASTC_4x4_KHR:37808|COMPRESSED_RGBA_ASTC_5x4_KHR:37809|COMPRESSED_RGBA_ASTC_5x5_KHR:37810|COMPRESSED_RGBA_ASTC_6x5_KHR:37811|COMPRESSED_RGBA_ASTC_6x6_KHR:37812|COMPRESSED_RGBA_ASTC_8x5_KHR:37813|COMPRESSED_RGBA_ASTC_8x6_KHR:37814|COMPRESSED_RGBA_ASTC_8x8_KHR:37815|COMPRESSED_RGBA_ASTC_10x5_KHR:37816|COMPRESSED_RGBA_ASTC_10x6_KHR:37817|COMPRESSED_RGBA_ASTC_10x8_KHR:37818|COMPRESSED_RGBA_ASTC_10x10_KHR:37819|COMPRESSED_RGBA_ASTC_12x10_KHR:37820|COMPRESSED_RGBA_ASTC_12x12_KHR:37821|COMPRESSED_SRGB8_ALPHA8_ASTC_4x4_KHR:37840|COMPRESSED_SRGB8_ALPHA8_ASTC_5x4_KHR:37841|COMPRESSED_SRGB8_ALPHA8_ASTC_5x5_KHR:37842|COMPRESSED_SRGB8_ALPHA8_ASTC_6x5_KHR:37843|COMPRESSED_SRGB8_ALPHA8_ASTC_6x6_KHR:37844|COMPRESSED_SRGB8_ALPHA8_ASTC_8x5_KHR:37845|COMPRESSED_SRGB8_ALPHA8_ASTC_8x6_KHR:37846|COMPRESSED_SRGB8_ALPHA8_ASTC_8x8_KHR:37847|COMPRESSED_SRGB8_ALPHA8_ASTC_10x5_KHR:37848|COMPRESSED_SRGB8_ALPHA8_ASTC_10x6_KHR:37849|COMPRESSED_SRGB8_ALPHA8_ASTC_10x8_KHR:37850|COMPRESSED_SRGB8_ALPHA8_ASTC_10x10_KHR:37851|COMPRESSED_SRGB8_ALPHA8_ASTC_12x10_KHR:37852|COMPRESSED_SRGB8_ALPHA8_ASTC_12x12_KHR:37853|getSupportedProfiles()0;' +
        'WEBGL_compressed_texture_etc=WebGLCompressedTextureETC|COMPRESSED_R11_EAC:37488|COMPRESSED_SIGNED_R11_EAC:37489|COMPRESSED_RG11_EAC:37490|COMPRESSED_SIGNED_RG11_EAC:37491|COMPRESSED_RGB8_ETC2:37492|COMPRESSED_SRGB8_ETC2:37493|COMPRESSED_RGB8_PUNCHTHROUGH_ALPHA1_ETC2:37494|COMPRESSED_SRGB8_PUNCHTHROUGH_ALPHA1_ETC2:37495|COMPRESSED_RGBA8_ETC2_EAC:37496|COMPRESSED_SRGB8_ALPHA8_ETC2_EAC:37497;' +
        'WEBGL_compressed_texture_etc1=WebGLCompressedTextureETC1|COMPRESSED_RGB_ETC1_WEBGL:36196;' +
        'WEBGL_compressed_texture_pvrtc=WebGLCompressedTexturePVRTC|COMPRESSED_RGB_PVRTC_4BPPV1_IMG:35840|COMPRESSED_RGB_PVRTC_2BPPV1_IMG:35841|COMPRESSED_RGBA_PVRTC_4BPPV1_IMG:35842|COMPRESSED_RGBA_PVRTC_2BPPV1_IMG:35843;' +
        'WEBGL_compressed_texture_s3tc=WebGLCompressedTextureS3TC|COMPRESSED_RGB_S3TC_DXT1_EXT:33776|COMPRESSED_RGBA_S3TC_DXT1_EXT:33777|COMPRESSED_RGBA_S3TC_DXT3_EXT:33778|COMPRESSED_RGBA_S3TC_DXT5_EXT:33779;' +
        'WEBGL_compressed_texture_s3tc_srgb=WebGLCompressedTextureS3TCsRGB|COMPRESSED_SRGB_S3TC_DXT1_EXT:35916|COMPRESSED_SRGB_ALPHA_S3TC_DXT1_EXT:35917|COMPRESSED_SRGB_ALPHA_S3TC_DXT3_EXT:35918|COMPRESSED_SRGB_ALPHA_S3TC_DXT5_EXT:35919;' +
        'WEBGL_debug_renderer_info=WebGLDebugRendererInfo|UNMASKED_VENDOR_WEBGL:37445|UNMASKED_RENDERER_WEBGL:37446;' +
        'WEBGL_debug_shaders=WebGLDebugShaders|getTranslatedShaderSource()1;' +
        'WEBGL_depth_texture=WebGLDepthTexture|UNSIGNED_INT_24_8_WEBGL:34042;' +
        'WEBGL_draw_buffers=WebGLDrawBuffers|COLOR_ATTACHMENT0_WEBGL:36064|COLOR_ATTACHMENT1_WEBGL:36065|COLOR_ATTACHMENT2_WEBGL:36066|COLOR_ATTACHMENT3_WEBGL:36067|COLOR_ATTACHMENT4_WEBGL:36068|COLOR_ATTACHMENT5_WEBGL:36069|COLOR_ATTACHMENT6_WEBGL:36070|COLOR_ATTACHMENT7_WEBGL:36071|COLOR_ATTACHMENT8_WEBGL:36072|COLOR_ATTACHMENT9_WEBGL:36073|COLOR_ATTACHMENT10_WEBGL:36074|COLOR_ATTACHMENT11_WEBGL:36075|COLOR_ATTACHMENT12_WEBGL:36076|COLOR_ATTACHMENT13_WEBGL:36077|COLOR_ATTACHMENT14_WEBGL:36078|COLOR_ATTACHMENT15_WEBGL:36079|DRAW_BUFFER0_WEBGL:34853|DRAW_BUFFER1_WEBGL:34854|DRAW_BUFFER2_WEBGL:34855|DRAW_BUFFER3_WEBGL:34856|DRAW_BUFFER4_WEBGL:34857|DRAW_BUFFER5_WEBGL:34858|DRAW_BUFFER6_WEBGL:34859|DRAW_BUFFER7_WEBGL:34860|DRAW_BUFFER8_WEBGL:34861|DRAW_BUFFER9_WEBGL:34862|DRAW_BUFFER10_WEBGL:34863|DRAW_BUFFER11_WEBGL:34864|DRAW_BUFFER12_WEBGL:34865|DRAW_BUFFER13_WEBGL:34866|DRAW_BUFFER14_WEBGL:34867|DRAW_BUFFER15_WEBGL:34868|MAX_COLOR_ATTACHMENTS_WEBGL:36063|MAX_DRAW_BUFFERS_WEBGL:34852|drawBuffersWEBGL()1;' +
        'WEBGL_lose_context=WebGLLoseContext|loseContext()0|restoreContext()0;' +
        'WEBGL_multi_draw=WebGLMultiDraw|multiDrawArraysInstancedWEBGL()8|multiDrawArraysWEBGL()6|multiDrawElementsInstancedWEBGL()9|multiDrawElementsWEBGL()7;' +
        'WEBGL_polygon_mode=WebGLPolygonMode|POLYGON_MODE_WEBGL:2880|POLYGON_OFFSET_LINE_WEBGL:10754|LINE_WEBGL:6913|FILL_WEBGL:6914|polygonModeWEBGL()2;' +
        'WEBGL_provoking_vertex=WebGLProvokingVertex|FIRST_VERTEX_CONVENTION_WEBGL:36429|LAST_VERTEX_CONVENTION_WEBGL:36430|PROVOKING_VERTEX_WEBGL:36431|provokingVertexWEBGL()1;' +
        'WEBGL_render_shared_exponent=WebGLRenderSharedExponent;' +
        'WEBGL_stencil_texturing=WebGLStencilTexturing|DEPTH_STENCIL_TEXTURE_MODE_WEBGL:37098|STENCIL_INDEX_WEBGL:6401'
    );

    // The methods whose effect the engine models; every other extension
    // method is a no-op.
    const _GL_EXT_BEHAVIOUR = {
        loseContext() { const gl = _glExtOwner.get(this); if (gl) _glLose(gl, true); },
        restoreContext() { const gl = _glExtOwner.get(this); if (gl) _glLose(gl, false); },
    };

    // One prototype per interface, shared by every context, with no
    // constructor and no interface object on the global, as in Chrome.
    // The prototypes are built on first use, after cleanup_bootstrap has
    // removed the masking helper from the global, so it is kept here.
    const _maskAsNative = globalThis._maskAsNative;
    const _glExtProtos = new Map();
    const _glExtProto = (spec) => {
        const [iface, ...members] = spec.split('|');
        if (_glExtProtos.has(iface)) return _glExtProtos.get(iface);
        const proto = {};
        const methods = [];
        for (const m of members) {
            const call = /^(\w+)\(\)(\d+)$/.exec(m);
            if (!call) {
                const [key, value] = m.split(':');
                Object.defineProperty(proto, key, _glConst(Number(value)));
                continue;
            }
            const [, key, len] = call;
            const own = _GL_EXT_BEHAVIOUR[key];
            const fn = { [key]() { if (own) own.call(this); } }[key];
            Object.defineProperty(fn, 'length', { value: Number(len), configurable: true });
            Object.defineProperty(proto, key, _glMethod(fn));
            methods.push(key);
        }
        // No global reaches these prototypes, so the cleanup sweep that
        // masks interface methods as native code never sees them.
        if (methods.length) _maskAsNative(proto, ...methods);
        Object.defineProperty(proto, Symbol.toStringTag,
            { value: iface, writable: false, enumerable: false, configurable: true });
        _glExtProtos.set(iface, proto);
        return proto;
    };
    const _GL_EXT_SPECS = new Map(_GL_EXTENSIONS.split(';').map((entry) => {
        const at = entry.indexOf('=');
        return [entry.slice(0, at), entry.slice(at + 1)];
    }));

    // The extension objects a context has handed out, and the context each
    // one belongs to.
    const _glExtCache = new WeakMap();
    const _glExtOwner = new WeakMap();
    const _glExtension = (gl, name) => {
        const spec = _GL_EXT_SPECS.get(name);
        if (!spec) return null;
        let cache = _glExtCache.get(gl);
        if (!cache) { cache = new Map(); _glExtCache.set(gl, cache); }
        if (!cache.has(name)) {
            const ext = Object.create(_glExtProto(spec));
            _glExtOwner.set(ext, gl);
            cache.set(name, ext);
        }
        return cache.get(name);
    };

    // WEBGL_lose_context. Measured in Chrome 147: a lost context answers
    // isContextLost() true, getError() CONTEXT_LOST_WEBGL once, and null
    // from getParameter(), getExtension() and getSupportedExtensions().
    const _glLost = new WeakSet();
    const _glLose = (gl, lost) => {
        if (lost && !_glLost.has(gl)) {
            _glLost.add(gl);
            _glRecordError(gl, 0x9242);
        } else if (!lost) {
            _glLost.delete(gl);
        }
    };
    const _glWhileAlive = (proto, name) => {
        const live = proto[name];
        Object.defineProperty(proto, name, _glMethod({ [name]() {
            return _glLost.has(this) ? null : live.apply(this, arguments);
        } }[name]));
        Object.defineProperty(proto[name], 'length', { value: live.length, configurable: true });
    };

    const _completeGLSurface = (C, webgl2, impl) => {
        const proto = C.prototype;
        for (const { name, num, webgl2Only } of _glTable(_GL_CONSTS)) {
            if (webgl2Only && !webgl2) continue;
            Object.defineProperty(proto, name, _glConst(num));
            Object.defineProperty(C, name, _glConst(num));
        }
        for (const { name, num, webgl2Only } of _glTable(_GL_METHODS)) {
            if (webgl2Only && !webgl2) continue;
            const own = impl[name];
            // Chrome's two interfaces never share a function object, so the
            // WebGL 2 copy of a WebGL 1 method forwards to it.
            const fn = typeof own !== 'function' ? _glStub(name)
                : impl === proto ? own
                : { [name]() { return own.apply(this, arguments); } }[name];
            Object.defineProperty(fn, 'length', { value: num, configurable: true });
            Object.defineProperty(proto, name, _glMethod(fn));
        }
    };

    // WebGL 2 only: the sample counts a renderable format supports. Chrome
    // reports every power of two from MAX_SAMPLES down to 1 for RGBA8
    // (measured [8, 4, 2, 1] with MAX_SAMPLES 8) and null, with
    // INVALID_ENUM, for anything that is not a renderbuffer SAMPLES query
    // on a colour- or depth-renderable format.
    const _GL_RENDERABLE = new Set([
        0x8058 /* RGBA8 */, 0x8051 /* RGB8 */, 0x8D62 /* RGB565 */, 0x8056 /* RGBA4 */,
        0x8057 /* RGB5_A1 */, 0x8059 /* RGB10_A2 */, 0x8C43 /* SRGB8_ALPHA8 */,
        0x8229 /* R8 */, 0x822B /* RG8 */, 0x81A5 /* DEPTH_COMPONENT16 */,
        0x81A6 /* DEPTH_COMPONENT24 */, 0x8CAC /* DEPTH_COMPONENT32F */,
        0x88F0 /* DEPTH24_STENCIL8 */, 0x8CAD /* DEPTH32F_STENCIL8 */, 0x8D48 /* STENCIL_INDEX8 */,
    ]);
    WebGL2RenderingContext.prototype.getInternalformatParameter = function getInternalformatParameter(target, internalformat, pname) {
        if (target !== 0x8D41 || pname !== 0x80A9 || !_GL_RENDERABLE.has(internalformat)) {
            _glRecordError(this, 0x0500);
            return null;
        }
        const gpu = WebGLRenderingContext._surfaceFor(this);
        const max = gpu.params[0x8D57];
        const counts = [];
        for (let n = max; n >= 1; n = Math.floor(n / 2)) counts.push(n);
        return new Int32Array(counts);
    };

    const _webgl1Impl = WebGLRenderingContext.prototype;
    const _webgl2Impl = Object.assign(Object.create(null),
        Object.fromEntries(Object.getOwnPropertyNames(_webgl1Impl)
            .map((n) => [n, Object.getOwnPropertyDescriptor(_webgl1Impl, n).value])),
        { getInternalformatParameter: WebGL2RenderingContext.prototype.getInternalformatParameter });
    _completeGLSurface(WebGLRenderingContext, false, _webgl1Impl);
    _completeGLSurface(WebGL2RenderingContext, true, _webgl2Impl);
    for (const C of [WebGLRenderingContext, WebGL2RenderingContext]) {
        for (const name of ['getParameter', 'getExtension', 'getSupportedExtensions']) {
            _glWhileAlive(C.prototype, name);
        }
    }

    // AudioContext + OfflineAudioContext
    // Simulates the pipeline commonly used for audio fingerprinting:
    //   OscillatorNode → DynamicsCompressorNode → destination
    
    class AudioNode extends EventTarget {
        constructor() { super(); }
        connect() {}
        disconnect() {}
    }

    class AudioScheduledSourceNode extends AudioNode {
        constructor() { super(); }
        start() {}
        stop() {}
    }

    class OscillatorNode extends AudioScheduledSourceNode {
        _type = "sine";
        constructor(context) {
            super();
            this._context = context;
            this.frequency = new AudioParam(440, context, v => { if (context._setOscFreq) context._setOscFreq(v); });
            this.detune = new AudioParam(0, context);
        }
        get type() { return this._type; }
        set type(v) { this._type = v; if (this._context._setOscType) this._context._setOscType(v); }
    }

    class AudioParam {
        constructor(val, context, setter) {
            this._value = val;
            this._context = context;
            this._setter = setter;
        }
        // Chrome stores AudioParam values as float32; the getter reads the
        // stored f32 back, so 0.003 reads as 0.003000000026077032.
        get value() { return Math.fround(this._value); }
        set value(v) { this._value = Math.fround(+v); if (this._setter) this._setter(this._value); }
        setValueAtTime(v, t) { this._value = Math.fround(+v); if (this._setter) this._setter(this._value); return this; }
        linearRampToValueAtTime() { return this; }
        exponentialRampToValueAtTime() { return this; }
        setTargetAtTime() { return this; }
        setValueCurveAtTime() { return this; }
        cancelScheduledValues() { return this; }
        cancelAndHoldAtTime() { return this; }
    }

    class GainNode extends AudioNode {
        constructor() {
            super();
            this.gain = new AudioParam(1);
        }
    }

    class DynamicsCompressorNode extends AudioNode {
        constructor(context) {
            super();
            this.threshold = new AudioParam(-24, context, v => { if (context._setCompThreshold) context._setCompThreshold(v); });
            this.knee = new AudioParam(30, context, v => { if (context._setCompKnee) context._setCompKnee(v); });
            this.ratio = new AudioParam(12, context, v => { if (context._setCompRatio) context._setCompRatio(v); });
            this.attack = new AudioParam(0.003, context, v => { if (context._setCompAttack) context._setCompAttack(v); });
            this.release = new AudioParam(0.25, context, v => { if (context._setCompRelease) context._setCompRelease(v); });
        }
    }

    class BiquadFilterNode extends AudioNode {
        constructor() {
            super();
            this.type = "lowpass";
            this.frequency = new AudioParam(350);
            this.detune = new AudioParam(0);
            this.Q = new AudioParam(1);
            this.gain = new AudioParam(0);
        }
        getFrequencyResponse(freqArr, magOut, phaseOut) {
            if (!(freqArr instanceof Float32Array)) return;
            const _typeIds = {
                lowpass: 0, highpass: 1, bandpass: 2, lowshelf: 3,
                highshelf: 4, peaking: 5, notch: 6, allpass: 7,
            };
            const tid = _typeIds[this.type] ?? 0;
            const sr = (this._sampleRate || 44100);
            const inBytes = new Uint8Array(freqArr.buffer, freqArr.byteOffset, freqArr.byteLength);
            const out = ops.op_audio_biquad_response(
                inBytes, tid,
                this.frequency.value, this.Q.value,
                this.gain.value, sr
            );
            const result = new Float32Array(out.buffer, out.byteOffset, out.byteLength / 4);
            const n = freqArr.length;
            const lenM = Math.min(magOut.length, n);
            const lenP = Math.min(phaseOut.length, n);
            for (let i = 0; i < lenM; i++) magOut[i] = result[i];
            for (let i = 0; i < lenP; i++) phaseOut[i] = result[n + i];
        }
    }

    class AnalyserNode extends AudioNode {
        constructor() {
            super();
            this.fftSize = 2048;
            this.smoothingTimeConstant = 0.8;
            this.minDecibels = -100;
            this.maxDecibels = -30;
            this._timeDomain = null;
            this._prevFreq = null;
        }
        get frequencyBinCount() { return this.fftSize / 2; }
        getByteFrequencyData(arr) {
            const f = new Float32Array(this.frequencyBinCount);
            this.getFloatFrequencyData(f);
            const range = this.maxDecibels - this.minDecibels;
            const len = Math.min(arr.length, f.length);
            for (let i = 0; i < len; i++) {
                const norm = (f[i] - this.minDecibels) / range;
                arr[i] = Math.max(0, Math.min(255, Math.round(norm * 255)));
            }
        }
        getFloatFrequencyData(arr) {
            if (!this._timeDomain || this._timeDomain.length < this.fftSize) {
                for (let i = 0; i < arr.length; i++) arr[i] = this.minDecibels;
                return;
            }
            const tdBytes = new Uint8Array(this._timeDomain.buffer, 0, this.fftSize * 4);
            const prevBytes = this._prevFreq
                ? new Uint8Array(this._prevFreq.buffer)
                : new Uint8Array(0);
            const out = ops.op_audio_analyser_freq_data(
                tdBytes, this.fftSize,
                Math.round(this.smoothingTimeConstant * 100),
                prevBytes
            );
            const result = new Float32Array(out.buffer, out.byteOffset, out.byteLength / 4);
            const len = Math.min(arr.length, result.length);
            for (let i = 0; i < len; i++) arr[i] = result[i];
            this._prevFreq = result.slice();
        }
        getByteTimeDomainData(arr) {
            if (!this._timeDomain) {
                for (let i = 0; i < arr.length; i++) arr[i] = 128;
                return;
            }
            const len = Math.min(arr.length, this._timeDomain.length);
            for (let i = 0; i < len; i++) {
                arr[i] = Math.max(0, Math.min(255, Math.round((this._timeDomain[i] + 1) * 127.5)));
            }
        }
        getFloatTimeDomainData(arr) {
            if (!this._timeDomain) {
                for (let i = 0; i < arr.length; i++) arr[i] = 0;
                return;
            }
            const len = Math.min(arr.length, this._timeDomain.length);
            for (let i = 0; i < len; i++) arr[i] = this._timeDomain[i];
        }
    }

    class AudioDestinationNode extends AudioNode {
        constructor() { super(); this.maxChannelCount = 2; }
    }

    // AudioContext fingerprintable surface. Real Chrome reports a
    // stable per-device value across page loads. Previously this used
    // `Math.random()` per-IIFE which made sequential page loads in the
    // same SharedSession return DIFFERENT sampleRates — an inconsistency
    // a real browser would not exhibit.
    //
    // Now: sampleRate reads from profile.audio_sample_rate (48000 on
    // Apple Silicon, 44100 elsewhere). baseLatency + outputLatency are
    // derived deterministically from `audio_seed` so they look like real
    // hardware variation but stay stable across page loads.
    const _audioSampleRate = (() => {
        try {
            const has = ops.op_has_stealth_profile && ops.op_has_stealth_profile();
            if (has) {
                const raw = ops.op_get_profile_value("audio_sample_rate");
                const v = parseInt(raw, 10);
                // Stealth profile validate() restricts this to
                // {44100, 48000, 96000, 192000}; we trust it here.
                if (Number.isInteger(v) && v > 0) return v;
            }
        } catch (_) {}
        return 48000;
    })();
    const _audioBaseLatency = (() => {
        // Real Chrome reports baseLatency in [0.005, 0.030] sec range
        // depending on output device. Derive deterministically from
        // bits 0-9 of audio_seed so it's stable per profile.
        let bits = 512; // mid-range fallback
        try {
            const has = ops.op_has_stealth_profile && ops.op_has_stealth_profile();
            if (has) {
                const raw = ops.op_get_profile_value("audio_seed");
                if (raw) {
                    bits = Number(BigInt(raw) & 0x3ffn); // 0..1023
                }
            }
        } catch (_) {}
        const v = 0.005 + (bits / 1023) * 0.025;
        return Math.round(v * 1000) / 1000;
    })();
    const _audioOutputLatency = (() => {
        // outputLatency > baseLatency typically. Add 5-30ms on top,
        // derived from bits 10-19 of audio_seed.
        let bits = 512;
        try {
            const has = ops.op_has_stealth_profile && ops.op_has_stealth_profile();
            if (has) {
                const raw = ops.op_get_profile_value("audio_seed");
                if (raw) {
                    bits = Number((BigInt(raw) >> 10n) & 0x3ffn);
                }
            }
        } catch (_) {}
        const v = _audioBaseLatency + 0.005 + (bits / 1023) * 0.025;
        return Math.round(v * 1000) / 1000;
    })();

    class BaseAudioContext extends EventTarget {
        constructor() {
            super();
            this.sampleRate = _audioSampleRate;
            this.baseLatency = _audioBaseLatency;
            this.outputLatency = _audioOutputLatency;
            this.state = "running";
            this.currentTime = 0;
            this.destination = new AudioDestinationNode();
            this.listener = {}; // AudioListener stub
        }
        createOscillator() { return new OscillatorNode(this); }
        createDynamicsCompressor() { return new DynamicsCompressorNode(this); }
        createAnalyser() { return new AnalyserNode(this); }
        createGain() { return new GainNode(this); }
        createBiquadFilter() { return new BiquadFilterNode(this); }
        createBufferSource() {
             return { connect() {}, start() {}, stop() {}, buffer: null, loop: false };
        }
        createBuffer(channels, length, sampleRate) {
            const bufs = [];
            for (let c = 0; c < channels; c++) bufs.push(new Float32Array(length));
            return {
                numberOfChannels: channels, length, sampleRate,
                duration: length / sampleRate,
                getChannelData(c) { return bufs[c]; }
            };
        }
        decodeAudioData() { return Promise.resolve(); }
        resume() { return Promise.resolve(); }
    }
    globalThis.BaseAudioContext = BaseAudioContext;

    class AudioContext extends BaseAudioContext {
        constructor() {
            super();
        }
        close() { return Promise.resolve(); }
        suspend() { return Promise.resolve(); }
    }

    class OfflineAudioContext extends BaseAudioContext {
        constructor(channels, length, sampleRate) {
            super();
            this._channels = channels || 1;
            this._length = length || _audioSampleRate;
            this.sampleRate = sampleRate || _audioSampleRate;
            this._oscType = "triangle";
            this._oscFreq = 10000;
            this._compThreshold = -24;
            this._compKnee = 30;
            this._compRatio = 12;
            this._compAttack = 0.003;
            this._compRelease = 0.25;
        }
        _setOscType(v) { this._oscType = v; }
        _setOscFreq(v) { this._oscFreq = v; }
        _setCompThreshold(v) { this._compThreshold = v; }
        _setCompKnee(v) { this._compKnee = v; }
        _setCompRatio(v) { this._compRatio = v; }
        _setCompAttack(v) { this._compAttack = v; }
        _setCompRelease(v) { this._compRelease = v; }

        startRendering() {
            const self = this;
            return new Promise((resolve) => {
                const sr = self.sampleRate;
                const len = self._length;
                const freq = self._oscFreq;
                const type = self._oscType;
                const waveTypeId = type === "sine" ? 0
                    : type === "square" ? 2
                    : type === "sawtooth" ? 3
                    : 1; // triangle

                let seed = 0;
                try {
                    // Use the local `ops` binding (same as canvas_seed path
                    // at line 59) — `Deno` may be removed by stealth cleanup,
                    // but `ops` was captured at IIFE entry.
                    if (ops.op_has_stealth_profile && ops.op_has_stealth_profile()) {
                        const raw = ops.op_get_profile_value("audio_seed");
                        if (raw) {
                            // op_get_profile_value returns u64 stringified.
                            // parseInt → Number lossy-coerces past 2^53, then
                            // `| 0` truncates a rounded float — distinct u64s
                            // can collapse to the same int32. BigInt.asIntN(32)
                            // does exact 32-bit truncation.
                            try {
                                seed = Number(BigInt.asIntN(32, BigInt(raw)));
                            } catch (_) {
                                const parsed = parseInt(raw, 10);
                                if (!Number.isNaN(parsed)) seed = parsed | 0;
                            }
                        }
                    }
                } catch (e) {}

                let data;
                try {
                    const bytes = ops.op_offline_audio_render(
                        seed, sr | 0, len | 0, freq, waveTypeId,
                        self._compThreshold, self._compKnee, self._compRatio,
                        self._compAttack, self._compRelease,
                    );
                    data = new Float32Array(bytes.buffer, bytes.byteOffset, len);
                } catch (e) {
                    data = new Float32Array(len);
                }

                const buf = {
                    numberOfChannels: self._channels,
                    length: len,
                    sampleRate: sr,
                    duration: len / sr,
                    getChannelData() { return data; },
                };
                resolve(buf);
            });
        }
    }

    // HTMLCanvasElement: getContext returns the right context
    class HTMLCanvasElement {
        #canvasId;
        #attrs;
        constructor(width = 300, height = 150) {
            this.#canvasId = null;
            this.#attrs = { width: String(width), height: String(height) };
            // Element base properties — fpCollect and bot.sannysoft expect these.
            // Use defineProperty because Element.prototype (which we chain into
            // at the bottom of this file) has tagName/nodeName/etc. as getters
            // with no setters — direct assignment would fail.
            Object.defineProperty(this, 'tagName', { value: 'CANVAS', configurable: true, writable: true });
            Object.defineProperty(this, 'nodeName', { value: 'CANVAS', configurable: true, writable: true });
            Object.defineProperty(this, 'nodeType', { value: 1, configurable: true, writable: true });
            Object.defineProperty(this, 'style', { value: { cssText: "" }, configurable: true, writable: true });
            Object.defineProperty(this, 'classList', {
                value: { add() {}, remove() {}, toggle() {}, contains() { return false; } },
                configurable: true, writable: true,
            });
            Object.defineProperty(this, 'dataset', { value: {}, configurable: true, writable: true });
            Object.defineProperty(this, 'childNodes', { value: [], configurable: true, writable: true });
            Object.defineProperty(this, 'children', { value: [], configurable: true, writable: true });
        }
        // Attribute API — required by canvas fingerprinters that do
        // `canvas.setAttribute('width', 200)` before drawing.
        setAttribute(name, value) {
            this.#attrs[name] = String(value);
            // Chrome: changing width/height clears and resizes the bitmap.
            if (name === "width" || name === "height") {
                this.#resizeFromAttrs();
            }
        }
        #resizeFromAttrs() {
            if (this.#canvasId) {
                ops.op_canvas_resize(
                    this.#canvasId,
                    parseInt(this.#attrs.width, 10) || 300,
                    parseInt(this.#attrs.height, 10) || 150,
                );
            }
        }
        get width() { return parseInt(this.#attrs.width, 10) || 300; }
        set width(v) { this.#attrs.width = String(Math.max(0, v | 0) || 300); this.#resizeFromAttrs(); }
        get height() { return parseInt(this.#attrs.height, 10) || 150; }
        set height(v) { this.#attrs.height = String(Math.max(0, v | 0) || 150); this.#resizeFromAttrs(); }
        getAttribute(name) { return this.#attrs[name] !== undefined ? this.#attrs[name] : null; }
        removeAttribute(name) { delete this.#attrs[name]; }
        hasAttribute(name) { return name in this.#attrs; }
        getContext(type) {
            if (!this.#canvasId) {
                this.#canvasId = ops.op_canvas_create(
                    parseInt(this.#attrs.width, 10) || 300,
                    parseInt(this.#attrs.height, 10) || 150,
                    _getOsName(), _getCanvasSeed());
            }
            if (type === "2d") return _context2d(this, this.#canvasId);
            if (type === "webgl" || type === "webgl2" || type === "experimental-webgl") {
                // FIX-D2: webgl2 → WebGL2RenderingContext (distinct class +
                // WebGL 2 surface); webgl/experimental-webgl → WebGLRenderingContext
                // with the WebGL 1 surface (_isWebGL2 = false).
                const isV2 = (type === "webgl2");
                const gl = isV2 ? new WebGL2RenderingContext() : new WebGLRenderingContext();
                gl._isWebGL2 = isV2;
                gl.canvas = this;
                gl.drawingBufferWidth = this.width;
                gl.drawingBufferHeight = this.height;
                return gl;
            }
            return null;
        }
        toDataURL(type, quality) {
            if (!this.#canvasId) {
                this.#canvasId = ops.op_canvas_create(parseInt(this.#attrs.width, 10) || 300, parseInt(this.#attrs.height, 10) || 150, _getOsName(), _getCanvasSeed());
            }
            const mime = typeof type === "string" && type.startsWith("image/") ? type : "image/png";
            return ops.op_canvas_to_data_url_typed(this.#canvasId, mime,
                typeof quality === "number" && Number.isFinite(quality) ? quality : null);
        }
        toBlob(cb, type) { cb(new Blob([this.toDataURL()])); }
        // Minimal Node API
        appendChild(child) { this.childNodes.push(child); return child; }
        removeChild(child) {
            const i = this.childNodes.indexOf(child);
            if (i >= 0) this.childNodes.splice(i, 1);
            return child;
        }
        addEventListener(type, listener, options) {
            // Inherit from Node -> EventTarget
            return super.addEventListener(type, listener, options);
        }
        removeEventListener(type, listener, options) {
            return super.removeEventListener(type, listener, options);
        }
        dispatchEvent(event) {
            return super.dispatchEvent(event);
        }
        // Clone / get bounding box — fingerprint probes may call these
        cloneNode() { return new HTMLCanvasElement(this.width, this.height); }
        getBoundingClientRect() {
            return { x: 0, y: 0, width: this.width, height: this.height, top: 0, left: 0, right: this.width, bottom: this.height };
        }
    }

    // Do NOT replace globalThis.HTMLCanvasElement — dom_bootstrap already
    // exposes it as a subclass of HTMLElement ← Element ← Node ← EventTarget.
    // Instead, chain our standalone canvas class's prototype to the dom
    // HTMLCanvasElement.prototype so `standalone instanceof HTMLCanvasElement`
    // returns true.
    //
    // Capture the DOM-side HTMLCanvasElement.prototype BEFORE the swap so we
    // can also install the lazy `_canvasId`-based methods (getContext,
    // toDataURL, ...) onto it. HTML-parsed <canvas> elements have THIS
    // prototype in their chain — not the standalone's — so without this
    // double install they would not see `getContext`. The lazy methods
    // installed further down work on both kinds of canvas (`_canvasId`
    // is initialised on demand via `_lazyInitCanvas`).
    // One 2D context per canvas: Chrome returns the same object on every
    // getContext('2d') call.
    function _context2d(canvas, id) {
        let ctx = _ctx2dByCanvas.get(canvas);
        if (!ctx) {
            ctx = new CanvasRenderingContext2D(id);
            _ctxOwner.set(ctx, canvas);
            _ctx2dByCanvas.set(canvas, ctx);
        }
        return ctx;
    }
    const _ctx2dByCanvas = new WeakMap();

    class OffscreenCanvasRenderingContext2D extends CanvasRenderingContext2D {}
    Object.defineProperty(OffscreenCanvasRenderingContext2D.prototype, Symbol.toStringTag, {
        value: "OffscreenCanvasRenderingContext2D", configurable: true,
    });
    globalThis.OffscreenCanvasRenderingContext2D = OffscreenCanvasRenderingContext2D;

    let _domCanvasProto = null;
    if (globalThis.HTMLCanvasElement) {
        _domCanvasProto = globalThis.HTMLCanvasElement.prototype;
        Object.setPrototypeOf(HTMLCanvasElement.prototype, globalThis.HTMLCanvasElement.prototype);
        Object.setPrototypeOf(HTMLCanvasElement, globalThis.HTMLCanvasElement);
    }
    globalThis.HTMLCanvasElement = HTMLCanvasElement;
    globalThis.CanvasRenderingContext2D = CanvasRenderingContext2D;
    globalThis.WebGLRenderingContext = WebGLRenderingContext;
    // Symbol.toStringTag — some scripts check
    // Object.prototype.toString.call(ctx) which must return
    // "[object CanvasRenderingContext2D]" / "[object WebGLRenderingContext]"
    // (not "[object Object]"). Without this tag we show as a bot.
    try {
        Object.defineProperty(CanvasRenderingContext2D.prototype, Symbol.toStringTag, {
            value: "CanvasRenderingContext2D",
            configurable: true,
        });
        Object.defineProperty(WebGLRenderingContext.prototype, Symbol.toStringTag, {
            value: "WebGLRenderingContext",
            configurable: true,
        });
        // FIX-D2: WebGL2RenderingContext is its own class now — give it its own
        // toStringTag so `Object.prototype.toString.call(gl2)` returns
        // "[object WebGL2RenderingContext]" (own prop shadows the inherited one).
        Object.defineProperty(WebGL2RenderingContext.prototype, Symbol.toStringTag, {
            value: "WebGL2RenderingContext",
            configurable: true,
        });
        Object.defineProperty(WebGLRenderingContext.prototype, 'constructor', {
            value: WebGLRenderingContext,
            configurable: true,
            writable: true,
        });
        Object.defineProperty(WebGL2RenderingContext.prototype, 'constructor', {
            value: WebGL2RenderingContext,
            configurable: true,
            writable: true,
        });
        Object.defineProperty(CanvasRenderingContext2D.prototype, 'constructor', {
            value: CanvasRenderingContext2D,
            configurable: true,
            writable: true,
        });
    } catch {}
    globalThis.WebGL2RenderingContext = WebGL2RenderingContext;
    globalThis.AudioContext = AudioContext;
    globalThis.OfflineAudioContext = OfflineAudioContext;
    globalThis.BaseAudioContext = BaseAudioContext;
    globalThis.webkitAudioContext = AudioContext;
    // Symbol.toStringTag for audio contexts — some scripts probe these.
    try {
        Object.defineProperty(AudioContext.prototype, Symbol.toStringTag, {
            value: "AudioContext", configurable: true,
        });
        Object.defineProperty(OfflineAudioContext.prototype, Symbol.toStringTag, {
            value: "OfflineAudioContext", configurable: true,
        });
        Object.defineProperty(BaseAudioContext.prototype, Symbol.toStringTag, {
            value: "BaseAudioContext", configurable: true,
        });
    } catch {}

    // Patch document.createElement to return HTMLCanvasElement for 'canvas'
    const _origCreateElement = globalThis.document?.createElement?.bind(globalThis.document);
    if (_origCreateElement) {
        const _origFn = globalThis.document.createElement;
        globalThis.document.createElement = function(tag) {
            if (tag.toLowerCase() === "canvas") return new HTMLCanvasElement();
            return _origFn.call(this, tag);
        };
    }

    // Install canvas-specific methods on `HTMLCanvasElement.prototype`
    // directly (NOT on Element.prototype). Real Chrome's DOM uses
    // WebIDL-generated bindings where `getContext` / `toDataURL` /
    // `toBlob` are own properties of HTMLCanvasElement.prototype with
    // brand-checking that throws `TypeError: Illegal invocation` when
    // called on a non-canvas `this`. Fingerprint probes check for
    // this via `Object.getOwnPropertyDescriptor(HTMLCanvasElement
    // .prototype, 'getContext')` and by calling methods with bogus
    // `this` to observe the error message.
    const _HTMLCanvasProto = globalThis.HTMLCanvasElement &&
        globalThis.HTMLCanvasElement.prototype;
    if (_HTMLCanvasProto) {
        // Brand-check helper: Chrome throws `TypeError: Illegal
        // invocation` with no stack-relevant info beyond the message.
        //
        // We accept either `tagName === "CANVAS"` (for HTML-parsed
        // canvases whose tag name is authoritative) or
        // `this instanceof HTMLCanvasElement` (for standalone
        // canvases from createElement whose constructor sets
        // tagName after assigning width/height). This matches the
        // shape probes fingerprinters actually run while allowing
        // partially-constructed canvases to pass the setter path.
        function _requireCanvas(self, methodName) {
            const ok =
                self &&
                (self.tagName === "CANVAS" ||
                    self instanceof globalThis.HTMLCanvasElement);
            if (!ok) {
                throw new __oxT.TypeError(
                    "Failed to execute '" +
                        methodName +
                        "' on 'HTMLCanvasElement': Illegal invocation"
                );
            }
        }
        function _lazyInitCanvas(self) {
            if (!self._canvasId) {
                const w = parseInt(self.getAttribute && self.getAttribute("width")) || 300;
                const h = parseInt(self.getAttribute && self.getAttribute("height")) || 150;
                self._canvasId = ops.op_canvas_create(w, h, _getOsName(), _getCanvasSeed());
            }
        }

        // canvas.width/height are IDL attributes: assignment writes the
        // content attribute AND resizes (clears) the backing bitmap —
        // Chrome semantics. A plain property assignment used to leave the
        // store at the 300x150 default, so every explicitly-sized canvas
        // rendered (and PNG-encoded) a 300x150 bitmap.
        Object.defineProperty(_HTMLCanvasProto, "width", {
            get() { return parseInt(this.getAttribute("width")) || 300; },
            set(v) {
                this.setAttribute("width", String(Math.max(0, v | 0) || 300));
                if (this._canvasId) ops.op_canvas_resize(this._canvasId, parseInt(this.getAttribute("width")), parseInt(this.getAttribute("height")) || 150);
            },
            enumerable: true, configurable: true,
        });
        Object.defineProperty(_HTMLCanvasProto, "height", {
            get() { return parseInt(this.getAttribute("height")) || 150; },
            set(v) {
                this.setAttribute("height", String(Math.max(0, v | 0) || 150));
                if (this._canvasId) ops.op_canvas_resize(this._canvasId, parseInt(this.getAttribute("width")) || 300, parseInt(this.getAttribute("height")));
            },
            enumerable: true, configurable: true,
        });

        Object.defineProperty(_HTMLCanvasProto, "getContext", {
            value: function getContext(type) {
                _requireCanvas(this, "getContext");
                _lazyInitCanvas(this);
                if (type === "2d") return _context2d(this, this._canvasId);
                if (
                    type === "webgl" ||
                    type === "webgl2" ||
                    type === "experimental-webgl"
                ) {
                    const w = parseInt(this.getAttribute("width")) || 300;
                    const h = parseInt(this.getAttribute("height")) || 150;
                    // FIX-D2: distinct class + surface per requested version.
                    const isV2 = (type === "webgl2");
                    const gl = isV2
                        ? new WebGL2RenderingContext(this._canvasId, w, h)
                        : new WebGLRenderingContext(this._canvasId, w, h);
                    gl._isWebGL2 = isV2;
                    gl.canvas = this;
                    return gl;
                }
                return null;
            },
            writable: true,
            configurable: true,
            enumerable: false,
        });

        Object.defineProperty(_HTMLCanvasProto, "toDataURL", {
            value: function toDataURL(type, quality) {
                _requireCanvas(this, "toDataURL");
                // Auto-allocate a canvas if none yet — real Chrome
                // serializes any HTMLCanvasElement, even one whose 2D
                // context was never requested. The result is a fully
                // transparent PNG of the element's width × height.
                if (!this._canvasId) {
                    try { this.getContext("2d"); } catch (_e) {}
                }
                if (!this._canvasId) return "data:,";
                const mime = typeof type === "string" && type.startsWith("image/") ? type : "image/png";
                return ops.op_canvas_to_data_url_typed(this._canvasId, mime,
                    typeof quality === "number" && Number.isFinite(quality) ? quality : null);
            },
            writable: true,
            configurable: true,
            enumerable: false,
        });

        Object.defineProperty(_HTMLCanvasProto, "toBlob", {
            value: function toBlob(cb, type) {
                _requireCanvas(this, "toBlob");
                if (typeof cb !== "function") {
                    throw new __oxT.TypeError(
                        "Failed to execute 'toBlob' on 'HTMLCanvasElement': callback is not a function"
                    );
                }
                // Match Chrome: the callback fires asynchronously on
                // the next microtask, not synchronously.
                const url = this._canvasId ? ops.op_canvas_to_data_url(this._canvasId) : "data:,";
                queueMicrotask(() => {
                    try {
                        cb(new Blob([url], { type: type || "image/png" }));
                    } catch (_e) {}
                });
            },
            writable: true,
            configurable: true,
            enumerable: false,
        });

        // Note: `width` and `height` are deliberately NOT installed on
        // the prototype here. The standalone canvas class in this
        // bootstrap sets them as own instance properties in its
        // constructor before `tagName` is defined, so adding a
        // brand-checking prototype setter breaks construction. A
        // prototype-level width/height accessor would also collide
        // with HTML-parsed `<canvas>` elements whose `getAttribute`
        // path is already canonical. Leave them as instance props.
    }

    // OffscreenCanvas — real canvas-backed implementation.
    //
    // Replaces the minimal stub from window_bootstrap.js (which had
    // `getContext() → null`). With canvas_ext already wired in for
    // the main thread and an identical bootstrap loading in workers,
    // `new OffscreenCanvas(w, h).getContext('2d')` now returns a
    // functional CanvasRenderingContext2D backed by the same ops the
    // on-DOM `<canvas>` element uses — real fillRect, real text,
    // real toDataURL.
    //
    // Anti-fingerprint sites probe this path via
    // `const ctx = new OffscreenCanvas(w, h).getContext('2d'); ctx.fillText(...)`.
    class RealOffscreenCanvas extends EventTarget {
        constructor(width, height) {
            super();
            this.width = width | 0;
            this.height = height | 0;
            this._canvasId = 0;
            this._context = null;
        }
        getContext(type, _opts) {
            if (type === "2d") {
                if (!this._canvasId) {
                    this._canvasId = ops.op_canvas_create(this.width, this.height, _getOsName(), _getCanvasSeed());
                }
                if (!this._context) {
                    this._context = new OffscreenCanvasRenderingContext2D(this._canvasId);
                    _ctxOwner.set(this._context, this);
                }
                return this._context;
            }
            if (type === "webgl" || type === "webgl2" || type === "experimental-webgl") {
                // FP parity: a real OffscreenCanvas exposes WebGL. Some
                // fingerprint workers read webGLVendor/webGLRenderer via
                // `new OffscreenCanvas(1,1).getContext('webgl')` →
                // gl.getParameter(UNMASKED_VENDOR_WEBGL); returning null here
                // differed from real Chrome (the on-DOM <canvas> already
                // supports WebGL).
                // Back it with the same profile-spoofed context that <canvas>
                // getContext uses (canvas_bootstrap.js:1232-1234).
                if (!this._canvasId) {
                    this._canvasId = ops.op_canvas_create(this.width, this.height, _getOsName(), _getCanvasSeed());
                }
                const _k = (type === "webgl2") ? "_glctx2" : "_glctx1";
                if (!this[_k]) {
                    const isV2 = (type === "webgl2");
                    const gl = isV2
                        ? new WebGL2RenderingContext(this._canvasId, this.width, this.height)
                        : new WebGLRenderingContext(this._canvasId, this.width, this.height);
                    gl._isWebGL2 = isV2;
                    gl.canvas = this;
                    this[_k] = gl;
                }
                return this[_k];
            }
            return null;
        }
        transferToImageBitmap() {
            if (!this._context && !this._glctx1 && !this._glctx2) {
                throw new __oxT.DOMException("Failed to execute 'transferToImageBitmap' on 'OffscreenCanvas': Cannot transfer an ImageBitmap from an OffscreenCanvas with no context", "InvalidStateError");
            }
            const bitmap = new globalThis.ImageBitmap();
            Object.defineProperty(bitmap, "width", { value: this.width, configurable: true });
            Object.defineProperty(bitmap, "height", { value: this.height, configurable: true });
            return bitmap;
        }
        async convertToBlob(options) {
            const type = (options && options.type) || "image/png";
            if (!this._canvasId) {
                return new Blob([], { type });
            }
            // toDataURL returns `data:<type>;base64,<data>` — strip
            // the prefix and decode to bytes for a real Blob body.
            const url = ops.op_canvas_to_data_url(this._canvasId);
            const comma = url.indexOf(",");
            if (comma < 0) return new Blob([], { type });
            const b64 = url.slice(comma + 1);
            const bin = typeof atob === "function" ? atob(b64) : "";
            const bytes = new Uint8Array(bin.length);
            for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
            return new Blob([bytes], { type });
        }
    }
    Object.defineProperty(RealOffscreenCanvas.prototype, Symbol.toStringTag, {
        value: "OffscreenCanvas",
        configurable: true,
    });
    // Install as the canonical global — overwrites the window_bootstrap stub.
    globalThis.OffscreenCanvas = RealOffscreenCanvas;

    // Mask methods as native
    if (typeof _maskAsNative === 'function') {
        _maskAsNative(CanvasRenderingContext2D.prototype, 
            'fillRect', 'strokeRect', 'clearRect', 'beginPath', 'moveTo', 'lineTo',
            'fill', 'stroke', 'closePath', 'arc', 'arcTo', 'bezierCurveTo',
            'quadraticCurveTo', 'rect', 'fillText', 'strokeText', 'measureText',
            'save', 'restore', 'translate', 'rotate', 'scale', 'setTransform',
            'resetTransform', 'getTransform', 'createLinearGradient', 
            'createRadialGradient', 'createPattern', 'getImageData', 'putImageData',
            'drawImage', 'isPointInPath', 'isPointInStroke');
        
        _maskAsNative(RealOffscreenCanvas.prototype, 'getContext', 'transferToImageBitmap', 'convertToBlob');

        // HTMLCanvasElement.prototype.transferControlToOffscreen — Chrome
        // 69+ method that returns a new OffscreenCanvas bound to this
        // element. Commonly probed as a real-Chrome
        // signal. Spec: https://html.spec.whatwg.org/#dom-canvas-transfercontroltooffscreen
        if (_HTMLCanvasProto && typeof _HTMLCanvasProto.transferControlToOffscreen !== "function") {
            const _transferControlToOffscreen = function transferControlToOffscreen() {
                const ok = this && (this.tagName === "CANVAS" ||
                    this instanceof globalThis.HTMLCanvasElement);
                if (!ok) {
                    throw new __oxT.TypeError(
                        "Failed to execute 'transferControlToOffscreen' on 'HTMLCanvasElement': Illegal invocation");
                }
                if (this._offscreenTransferred) {
                    throw new __oxT.DOMException(
                        "Cannot transfer control from a canvas for more than one time.",
                        "InvalidStateError");
                }
                const w = this.width || 300;
                const h = this.height || 150;
                this._offscreenTransferred = true;
                return new RealOffscreenCanvas(w, h);
            };
            Object.defineProperty(_HTMLCanvasProto, "transferControlToOffscreen", {
                value: _transferControlToOffscreen, configurable: true, writable: true,
            });
            try { _maskAsNative(_HTMLCanvasProto, 'transferControlToOffscreen'); } catch (_) {}
        }

        if (_HTMLCanvasProto) {
            _maskAsNative(_HTMLCanvasProto, 'getContext', 'toDataURL', 'toBlob');
        }

        // Mirror the lazy-init canvas methods onto the DOM-side
        // HTMLCanvasElement.prototype too. HTML-parsed <canvas> elements
        // returned by `document.getElementById(...)` have that prototype
        // in their chain — not the standalone one — so without this
        // mirror, `elem.getContext` is `undefined` on every parsed canvas.
        // The standalone methods read `this._canvasId` (initialised lazily
        // via `_lazyInitCanvas`), which works for both kinds of canvas.
        if (_domCanvasProto && _domCanvasProto !== _HTMLCanvasProto) {
            for (const name of ['getContext', 'toDataURL', 'toBlob', 'transferControlToOffscreen']) {
                const desc = Object.getOwnPropertyDescriptor(_HTMLCanvasProto, name);
                if (desc && !Object.getOwnPropertyDescriptor(_domCanvasProto, name)) {
                    Object.defineProperty(_domCanvasProto, name, desc);
                }
            }
        }

        if (globalThis.AudioContext) {
            _maskAsNative(AudioContext.prototype, 'createOscillator', 'createDynamicsCompressor', 'close', 'suspend', 'resume');
        }
        if (globalThis.OfflineAudioContext) {
            _maskAsNative(OfflineAudioContext.prototype, 'startRendering');
        }
        if (globalThis.BaseAudioContext) {
            _maskAsNative(BaseAudioContext.prototype, 'createOscillator', 'createDynamicsCompressor', 'createAnalyser', 'createGain', 'createBiquadFilter');
        }
        
        // Mask every own-function method on WebGL[2]RenderingContext.prototype.
        // Many scripts inspect Function.prototype.toString of
        // these methods, which must serialize as native code. Iterating
        // the prototype's own names is durable as the engine grows method
        // coverage — every new method gets masked automatically.
        const _maskAllProtoFns = (proto) => {
            if (!proto) return;
            const names = [];
            for (const n of Object.getOwnPropertyNames(proto)) {
                if (n === 'constructor') continue;
                const d = Object.getOwnPropertyDescriptor(proto, n);
                if (d && typeof d.value === 'function') names.push(n);
            }
            if (names.length) _maskAsNative(proto, ...names);
        };
        if (globalThis.WebGLRenderingContext) {
            _maskAllProtoFns(globalThis.WebGLRenderingContext.prototype);
        }
        if (globalThis.WebGL2RenderingContext) {
            _maskAllProtoFns(globalThis.WebGL2RenderingContext.prototype);
        }
    }
})(globalThis);
