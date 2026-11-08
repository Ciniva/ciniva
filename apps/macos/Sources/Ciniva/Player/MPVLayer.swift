import AppKit
import CMPV
import OpenGL.GL3

private let openGLBundle = CFBundleGetBundleWithIdentifier("com.apple.opengl" as CFString)

private func mpvGetProcAddress(_ ctx: UnsafeMutableRawPointer?, _ name: UnsafePointer<CChar>?) -> UnsafeMutableRawPointer? {
    guard let name, let sym = CFStringCreateWithCString(kCFAllocatorDefault, name, CFStringBuiltInEncodings.ASCII.rawValue) else {
        return nil
    }
    return CFBundleGetFunctionPointerForName(openGLBundle, sym)
}

final class MPVVideoView: NSView {
    let mpvLayer: MPVLayer
    var onScreenChange: (() -> Void)?

    init(mpv: OpaquePointer) {
        mpvLayer = MPVLayer(mpv: mpv)
        super.init(frame: .zero)
        wantsLayer = true
        layerContentsRedrawPolicy = .duringViewResize
    }

    required init?(coder: NSCoder) { fatalError() }

    override func makeBackingLayer() -> CALayer { mpvLayer }

    override var isOpaque: Bool { true }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        mpvLayer.forceRedraw()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        mpvLayer.contentsScale = window?.backingScaleFactor ?? 2
        mpvLayer.forceRedraw()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        NotificationCenter.default.removeObserver(self, name: NSWindow.didChangeScreenNotification, object: nil)
        guard let window else { return }
        mpvLayer.contentsScale = window.backingScaleFactor
        mpvLayer.setNeedsDisplay()
        NotificationCenter.default.addObserver(self, selector: #selector(screenChanged), name: NSWindow.didChangeScreenNotification, object: window)
        onScreenChange?()
    }

    @objc private func screenChanged() {
        onScreenChange?()
    }
}

final class MPVLayer: CAOpenGLLayer {
    private let mpv: OpaquePointer
    private var renderContext: OpaquePointer?
    private var cglContext: CGLContextObj?
    private let renderQueue = DispatchQueue(label: "ciniva.mpv.render", qos: .userInteractive)
    private var needsForcedRender = true
    var onReady: (() -> Void)?
    var isReady: Bool { renderContext != nil }

    init(mpv: OpaquePointer) {
        self.mpv = mpv
        super.init()
        isAsynchronous = false
        isOpaque = true
        backgroundColor = NSColor.black.cgColor
        needsDisplayOnBoundsChange = true
        autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
        colorspace = CGColorSpace(name: CGColorSpace.sRGB)
    }

    override init(layer: Any) {
        let other = layer as! MPVLayer
        mpv = other.mpv
        super.init(layer: layer)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func copyCGLPixelFormat(forDisplayMask mask: UInt32) -> CGLPixelFormatObj {
        let base: [CGLPixelFormatAttribute] = [
            kCGLPFAOpenGLProfile, CGLPixelFormatAttribute(kCGLOGLPVersion_3_2_Core.rawValue),
            kCGLPFAAccelerated,
            kCGLPFADoubleBuffer,
            kCGLPFAAllowOfflineRenderers,
        ]
        let hdr: [CGLPixelFormatAttribute] = [kCGLPFAColorFloat, kCGLPFAColorSize, CGLPixelFormatAttribute(64)]
        for attrs in [base + hdr, base] {
            var pix: CGLPixelFormatObj?
            var count: GLint = 0
            if CGLChoosePixelFormat(attrs + [CGLPixelFormatAttribute(0)], &pix, &count) == kCGLNoError, let pix {
                return pix
            }
        }
        return super.copyCGLPixelFormat(forDisplayMask: mask)
    }

    override func copyCGLContext(forPixelFormat pf: CGLPixelFormatObj) -> CGLContextObj {
        let ctx = super.copyCGLContext(forPixelFormat: pf)
        var swapInterval: GLint = 1
        CGLSetParameter(ctx, kCGLCPSwapInterval, &swapInterval)
        CGLEnable(ctx, kCGLCEMPEngine)
        CGLSetCurrentContext(ctx)
        cglContext = ctx
        createRenderContext()
        return ctx
    }

    private func createRenderContext() {
        guard renderContext == nil else { return }
        let apiType = strdup(MPV_RENDER_API_TYPE_OPENGL)
        defer { free(apiType) }
        var initParams = mpv_opengl_init_params(get_proc_address: mpvGetProcAddress, get_proc_address_ctx: nil)
        withUnsafeMutablePointer(to: &initParams) { initPtr in
            var params = [
                mpv_render_param(type: MPV_RENDER_PARAM_API_TYPE, data: apiType),
                mpv_render_param(type: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, data: initPtr),
                mpv_render_param(),
            ]
            var rc: OpaquePointer?
            if mpv_render_context_create(&rc, mpv, &params) >= 0 {
                renderContext = rc
            }
        }
        guard let renderContext else { return }
        mpv_render_context_set_update_callback(renderContext, { ctx in
            guard let ctx else { return }
            Unmanaged<MPVLayer>.fromOpaque(ctx).takeUnretainedValue().scheduleDraw()
        }, Unmanaged.passUnretained(self).toOpaque())
        DispatchQueue.main.async { [weak self] in
            self?.onReady?()
            self?.onReady = nil
        }
    }

    override func canDraw(inCGLContext ctx: CGLContextObj, pixelFormat pf: CGLPixelFormatObj, forLayerTime t: CFTimeInterval, displayTime ts: UnsafePointer<CVTimeStamp>?) -> Bool {
        guard let renderContext else { return false }
        let flags = mpv_render_context_update(renderContext)
        return needsForcedRender || (flags & UInt64(MPV_RENDER_UPDATE_FRAME.rawValue)) != 0
    }

    override func draw(inCGLContext ctx: CGLContextObj, pixelFormat pf: CGLPixelFormatObj, forLayerTime t: CFTimeInterval, displayTime ts: UnsafePointer<CVTimeStamp>?) {
        needsForcedRender = false
        glClearColor(0, 0, 0, 1)
        glClear(GLbitfield(GL_COLOR_BUFFER_BIT))
        guard let renderContext else { return }

        var fboID: GLint = 0
        glGetIntegerv(GLenum(GL_DRAW_FRAMEBUFFER_BINDING), &fboID)
        var viewport = [GLint](repeating: 0, count: 4)
        glGetIntegerv(GLenum(GL_VIEWPORT), &viewport)

        var fbo = mpv_opengl_fbo(fbo: fboID, w: viewport[2], h: viewport[3], internal_format: 0)
        var flipY: Int32 = 1
        withUnsafeMutablePointer(to: &fbo) { fboPtr in
            withUnsafeMutablePointer(to: &flipY) { flipPtr in
                var params = [
                    mpv_render_param(type: MPV_RENDER_PARAM_OPENGL_FBO, data: fboPtr),
                    mpv_render_param(type: MPV_RENDER_PARAM_FLIP_Y, data: flipPtr),
                    mpv_render_param(),
                ]
                mpv_render_context_render(renderContext, &params)
            }
        }
        glFlush()
    }

    override func display() {
        super.display()
        CATransaction.flush()
    }

    func scheduleDraw() {
        renderQueue.async { [weak self] in self?.display() }
    }

    func forceRedraw() {
        needsForcedRender = true
        scheduleDraw()
    }

    func setColorMode(hdr: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        wantsExtendedDynamicRangeContent = hdr
        colorspace = CGColorSpace(name: hdr ? CGColorSpace.itur_2100_PQ : CGColorSpace.sRGB)
        CATransaction.commit()
        forceRedraw()
    }

    func destroyRenderContext() {
        renderQueue.sync {
            guard let rc = renderContext else { return }
            mpv_render_context_set_update_callback(rc, nil, nil)
            if let cglContext {
                CGLLockContext(cglContext)
                CGLSetCurrentContext(cglContext)
                renderContext = nil
                mpv_render_context_free(rc)
                CGLUnlockContext(cglContext)
            } else {
                renderContext = nil
                mpv_render_context_free(rc)
            }
        }
    }
}
