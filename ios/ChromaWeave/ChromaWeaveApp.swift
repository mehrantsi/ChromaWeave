import SwiftUI
import PhotosUI
import AVFoundation
import WebKit
import UIKit
import Foundation

private enum Design {
    static let accent = Color(red: 0.08, green: 0.39, blue: 0.36)
    static let paper = Color(uiColor: .systemGroupedBackground)
    static let ink = Color.primary
}

@main
struct ChromaWeaveApp: App {
    init() {
        #if CW_DEVELOPMENT
        if ProcessInfo.processInfo.arguments.contains("--reset-diagnostics"),
           let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first {
            let report = AppOwnedDiagnosticCleanup.reset(documents: documents)
            if !report.succeeded { NSLog("Diagnostic reset incomplete. %@", report.failures.joined(separator: "; ")) }
        }
        #endif
    }

    var body: some Scene {
        WindowGroup {
            TabView {
                ScanView().tabItem { Label("Scan", systemImage: "viewfinder") }
                CreateView().tabItem { Label("Create", systemImage: "square.and.pencil") }
            }.tint(Design.accent)
        }
    }
}

private struct Eyebrow: View {
    let text: String
    var body: some View { Text(text.uppercased()).font(.caption2.weight(.semibold)).tracking(1.6).foregroundStyle(.secondary) }
}

private struct PrimaryButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.font(.body.weight(.semibold)).frame(maxWidth: .infinity).padding(.vertical, 16)
            .foregroundStyle(.white).background(Design.accent.opacity(configuration.isPressed ? 0.75 : 1), in: RoundedRectangle(cornerRadius: 15))
    }
}

private struct CameraPreview: UIViewRepresentable {
    let session: AVCaptureSession
    let enabled: Bool
    let onFocus: (CGPoint) -> Void
    final class Preview: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
        var onFocus: ((CGPoint) -> Void)?
        private let focusMarker = UIView(frame: CGRect(x: 0, y: 0, width: 64, height: 64))

        func enableFocus() {
            focusMarker.isUserInteractionEnabled = false
            focusMarker.layer.borderWidth = 1.5
            focusMarker.layer.borderColor = UIColor.white.cgColor
            focusMarker.layer.cornerRadius = 10
            focusMarker.alpha = 0
            addSubview(focusMarker)
            addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped(_:))))
        }

        @objc private func tapped(_ gesture: UITapGestureRecognizer) {
            let location = gesture.location(in: self)
            let videoRect = previewLayer.layerRectConverted(fromMetadataOutputRect: CGRect(x: 0, y: 0, width: 1, height: 1))
            guard videoRect.contains(location) else { return }
            onFocus?(previewLayer.captureDevicePointConverted(fromLayerPoint: location))
            focusMarker.layer.removeAllAnimations()
            focusMarker.center = location
            focusMarker.alpha = 1
            UIView.animate(withDuration: 0.3, delay: 0.7, options: .beginFromCurrentState) { self.focusMarker.alpha = 0 }
        }
    }
    func makeUIView(context: Context) -> Preview {
        let view = Preview()
        view.backgroundColor = .black
        view.previewLayer.session = session
        view.previewLayer.videoGravity = .resizeAspect
        view.enableFocus()
        return view
    }
    func updateUIView(_ view: Preview, context: Context) {
        view.onFocus = onFocus
        view.isUserInteractionEnabled = enabled
        if let connection = view.previewLayer.connection, connection.isVideoRotationAngleSupported(90) { connection.videoRotationAngle = 90 }
    }
}

private struct ScanView: View {
    #if CW_DEVELOPMENT
    @StateObject private var scanner = CameraScanner(configuration: DevelopmentReplay.configuration())
    #else
    @StateObject private var scanner = CameraScanner()
    #endif
    @State private var photo: PhotosPickerItem?
    @State private var photoPickerPresented = false
    @State private var scanMode: ScanMode = ProcessInfo.processInfo.arguments.contains("--photo-mode") ? .photo :
        ProcessInfo.processInfo.arguments.contains("--transfer-mode") ? .transfer : .live
    private var photoCaptureMode: Bool { scanMode == .photo }
    @State private var visible = false
    #if CW_DEVELOPMENT
    @State private var selfTestStarted = false
    #endif
    @State private var loadTask: Task<Void, Never>?
    @State private var diagnosticsData = Data()
    @State private var preparingDiagnostics = false
    @State private var exportingDiagnostics = false
    @State private var diagnosticsExportError: String?
    @State private var previousIdleTimerDisabled: Bool?
    @Environment(\.scenePhase) private var scenePhase
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    VStack(alignment: .leading, spacing: 8) {
                        Eyebrow(text: "On-device decoding")
                        Text("Scan CW Code").font(.system(size: 34, weight: .semibold, design: .serif)).fixedSize(horizontal: false, vertical: true)
                    }
                    Picker("Scanning mode", selection: $scanMode) {
                        ForEach(ScanMode.allCases) { mode in Text(mode.rawValue).tag(mode) }
                    }.pickerStyle(.segmented).disabled(scanner.processingImage)
                        .onChange(of: scanMode) { _, mode in scanner.setScanMode(mode) }
                    if photoCaptureMode {
                        Picker("Camera", selection: Binding(get: { scanner.photoCamera }, set: { scanner.setPhotoCamera($0) })) {
                            ForEach(PhotoCamera.allCases) { camera in
                                Text(camera.title).tag(camera).disabled(camera == .main && !scanner.mainCameraAvailable)
                            }
                        }.pickerStyle(.menu).disabled(scanner.processingImage)
                    }
                    if scanMode == .transfer {
                        VStack(alignment: .leading, spacing: 8) {
                            TimelineView(.periodic(from: .now, by: 0.25)) { _ in
                                HStack(alignment: .firstTextBaseline, spacing: 6) {
                                    Text(scanner.liveTransferMbps(at: CACurrentMediaTime()).map { String(format: "%.2f", $0) } ?? "—")
                                        .font(.title2.weight(.medium).monospacedDigit())
                                    Text("Mbit/s live").font(.subheadline).foregroundStyle(.secondary)
                                }
                            }
                            Text("Unique bytes · last 2 seconds").font(.caption).foregroundStyle(.secondary)
                            if let progress = scanner.transferProgress { ProgressView(value: progress) }
                            if !scanner.transferSummary.isEmpty {
                                Text(scanner.transferSummary).font(.caption.monospacedDigit())
                            }
                        }
                    }
                    ZStack {
                        CameraPreview(session: scanner.session, enabled: scanner.liveCamera) { point in
                            if scanner.liveCamera { scanner.focus(at: point) }
                        }
                        if !scanner.cameraAvailable {
                            VStack(spacing: 12) {
                                Image(systemName: "viewfinder").font(.system(size: 38, weight: .ultraLight))
                                Text("Camera unavailable\nImport a code image to scan.").font(.subheadline).multilineTextAlignment(.center)
                            }.foregroundStyle(.white.opacity(0.85)).padding(24)
                        }
                    }
                    .aspectRatio(scanner.cameraAvailable ? 1 : 1.65, contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: 20))
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(alignment: .top, spacing: 10) {
                            if scanner.processingImage { ProgressView().padding(.top, 3) }
                            else { Circle().fill(scanner.liveCamera ? Design.accent : Color.secondary).frame(width: 7, height: 7).padding(.top, 7) }
                            Text(scanner.status).font(.subheadline.weight(.medium))
                        }
                        if scanner.liveCamera {
                            Text(scanMode == .transfer ? "Point at the animated transfer. Keep the whole border visible; missing chunks can arrive on the next cycle." : photoCaptureMode ? "Preview only; no automatic decoding. Keep the complete code in view and tap it to focus." : "Keep the entire border in view. The camera combines additional frames when needed. Tap the code to focus.").font(.subheadline).foregroundStyle(.secondary)
                            HStack {
                                Button("Refocus", systemImage: "viewfinder") { scanner.refocus() }
                                Button("Capture frame", systemImage: "camera") { scanner.captureFrame() }
                            }.buttonStyle(.bordered)
                            Text(photoCaptureMode ? scanner.focusStatus : "Main camera · \(scanner.focusStatus)").font(.caption).foregroundStyle(.secondary)
                            if !scanner.photoFocusGuidance.isEmpty {
                                Text(scanner.photoFocusGuidance).font(.caption).foregroundStyle(.secondary)
                            }
                            if scanMode == .transfer {
                                Button("Restart transfer", systemImage: "arrow.clockwise") { scanner.restartTransfer() }
                            }
                            if photoCaptureMode && scanner.photoAvailable {
                                Button("Scan photo", systemImage: "camera.aperture") { scanner.scanPhoto() }
                                    .buttonStyle(PrimaryButton())
                                Text("For dense codes, hold still for a higher-resolution capture. \(scanner.photoResolution). Actual resolution depends on the active camera.")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    Button {
                        scanner.stop()
                        photoPickerPresented = true
                    } label: { Label("Read an image", systemImage: "photo") }
                        .buttonStyle(PrimaryButton()).disabled(scanner.processingImage || preparingDiagnostics)
                    HStack {
                        Menu {
                            ForEach(CodeSample.allCases) { kind in Button(kind.title) { scanner.sample(kind) } }
                        } label: { Label("Try a sample", systemImage: "sparkle.magnifyingglass") }
                        Spacer()
                        if scanner.cameraAvailable && !scanner.liveCamera && !scanner.processingImage {
                            Button("Resume camera") { scanner.resume() }
                        }
                    }.font(.subheadline).disabled(scanner.processingImage || preparingDiagnostics)
                    Divider()
                    Text("Images are decoded on this device. Scan photo saves its image and diagnostics on this iPhone. When a live scan fails, up to four diagnostic frames are retained on this iPhone.")
                        .font(.footnote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    DisclosureGroup("Scan details") {
                        VStack(alignment: .leading, spacing: 12) {
                            Text(scanner.diagnostic).font(.caption.monospaced()).textSelection(.enabled)
                            Text("Recent decoder and focus details are saved locally. Diagnostics contain no camera images or payload contents.")
                            Button(scanner.diagnosticsExportTitle, systemImage: "square.and.arrow.up", action: exportDiagnostics)
                                .disabled(preparingDiagnostics)
                            if let diagnosticsExportError { Text(diagnosticsExportError).foregroundStyle(.red) }
                        }.frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8)
                    }.font(.caption).foregroundStyle(.secondary)
                }.padding(24)
            }
            .background(Design.paper)
            .navigationTitle("CW Code").navigationBarTitleDisplayMode(.inline)
            .onAppear {
                visible = true
                scanner.setScanMode(scanMode)
                updateIdleTimer()
                #if CW_DEVELOPMENT
                if !selfTestStarted && DevelopmentReplay.start(scanner: scanner) { selfTestStarted = true }
                else if scenePhase == .active && !photoPickerPresented && photo == nil && !preparingDiagnostics && !exportingDiagnostics { scanner.start() }
                #else
                if scenePhase == .active && !photoPickerPresented && photo == nil && !preparingDiagnostics && !exportingDiagnostics { scanner.start() }
                #endif
            }
            .onDisappear {
                visible = false
                loadTask?.cancel()
                scanner.stop()
                updateIdleTimer()
            }
            .onChange(of: scenePhase) { _, phase in
                #if CW_DEVELOPMENT
                if selfTestStarted { updateIdleTimer(); return }
                #endif
                if phase == .active && visible && !photoPickerPresented && photo == nil && !preparingDiagnostics && !exportingDiagnostics { scanner.start() } else { scanner.stop() }
                updateIdleTimer()
            }
            .photosPicker(isPresented: $photoPickerPresented, selection: $photo, matching: .images)
            .onChange(of: photoPickerPresented) { _, presented in
                if !presented && photo == nil && visible && scenePhase == .active { scanner.start() }
                updateIdleTimer()
            }
            .onChange(of: photo) { _, item in
                updateIdleTimer()
                guard let item else { return }
                loadTask?.cancel()
                loadTask = Task {
                    let data = try? await item.loadTransferable(type: Data.self)
                    guard !Task.isCancelled, visible else { return }
                    if let data, let image = UIImage(data: data) { scanner.importImage(image) }
                    else { scanner.importFailed() }
                    photo = nil
                }
            }
            .onChange(of: scanner.liveCamera) { _, _ in updateIdleTimer() }
            .onChange(of: scanner.processingImage) { _, _ in updateIdleTimer() }
            .onChange(of: scanner.result != nil) { _, _ in updateIdleTimer() }
            .onChange(of: scanner.capturedFrame != nil) { _, _ in updateIdleTimer() }
            .onChange(of: preparingDiagnostics) { _, _ in updateIdleTimer() }
            .onChange(of: exportingDiagnostics) { _, _ in updateIdleTimer() }
            .sheet(item: $scanner.result, onDismiss: resume) { payload in
                PayloadView(payload: payload) { scanner.payloadSheetAppeared(for: payload) }
            }
            .sheet(item: $scanner.capturedFrame, onDismiss: resume) { CameraFrameView(frame: $0) }
            .fileExporter(isPresented: $exportingDiagnostics, document: BinaryDocument(data: diagnosticsData), contentType: .json,
                          defaultFilename: scanner.diagnosticsFilename) {
                if case .failure(let error) = $0 { diagnosticsExportError = error.localizedDescription }
            }
            .onChange(of: exportingDiagnostics) { _, presented in if !presented { resume() } }
        }
    }
    @MainActor private func updateIdleTimer() {
        let keepAwake = visible
            && scenePhase == .active
            && !photoPickerPresented
            && photo == nil
            && scanner.result == nil
            && scanner.capturedFrame == nil
            && !preparingDiagnostics
            && !exportingDiagnostics
            && (scanner.liveCamera || scanner.processingImage)
        if keepAwake {
            if previousIdleTimerDisabled == nil {
                previousIdleTimerDisabled = UIApplication.shared.isIdleTimerDisabled
            }
            UIApplication.shared.isIdleTimerDisabled = true
        } else if let previousIdleTimerDisabled {
            UIApplication.shared.isIdleTimerDisabled = previousIdleTimerDisabled
            self.previousIdleTimerDisabled = nil
        }
    }
    private func resume() { if visible && scenePhase == .active && !photoPickerPresented && photo == nil && !preparingDiagnostics && !exportingDiagnostics { scanner.resume() } }
    private func exportDiagnostics() {
        preparingDiagnostics = true
        diagnosticsExportError = nil
        scanner.stop()
        scanner.exportDiagnostics { result in
            preparingDiagnostics = false
            guard visible && scenePhase == .active else { return }
            switch result {
            case .success(let data):
                diagnosticsData = data
                exportingDiagnostics = true
            case .failure(let error):
                diagnosticsExportError = error.localizedDescription
                resume()
            }
        }
    }
}

private struct CameraFrameView: View {
    let frame: CapturedFrame
    @Environment(\.dismiss) private var dismiss
    @State private var png = Data()
    @State private var exporting = false
    @State private var diagnosticsData = Data()
    @State private var exportingDiagnostics = false
    @State private var preparingPNG = false
    @State private var payload: DecodedPayload?
    @State private var error: String?
    private var title: String { frame.isPhoto ? "Captured photo" : "Captured frame" }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    Image(uiImage: frame.image).resizable().scaledToFit().clipShape(RoundedRectangle(cornerRadius: 12))
                    Eyebrow(text: "\(frame.width) × \(frame.height) · \(frame.isPhoto ? "processed still" : "camera frame")")
                    Text(title).font(.title2.weight(.semibold))
                    if let details = frame.captureDetails { Text(details).font(.subheadline).foregroundStyle(.secondary) }
                    if let decoded = frame.decodedPayload {
                        Label("\(decoded.data.count.formatted()) bytes recovered", systemImage: "checkmark.circle.fill").foregroundStyle(Design.accent)
                        Button("View payload", systemImage: "doc.text") { payload = decoded }.buttonStyle(.bordered)
                    } else if let failure = frame.decodeError {
                        Text("No CW payload recovered").font(.subheadline.weight(.semibold))
                        Text(failure).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    if frame.savedOnDevice {
                        Label("Saved on this iPhone", systemImage: "checkmark.circle").font(.subheadline).foregroundStyle(Design.accent)
                    } else if let failure = frame.saveError {
                        Text("Could not save on this iPhone: \(failure). You can still export the image and diagnostics below.").font(.footnote).foregroundStyle(.red)
                    }
                    Text("The image and its diagnostic snapshot are frozen. Export both to inspect the scan or compare another decoder.").foregroundStyle(.secondary)
                    Button(preparingPNG ? "Preparing PNG…" : "Export \(frame.isPhoto ? "photo" : "frame") PNG", systemImage: "square.and.arrow.up") {
                        preparingPNG = true
                        error = nil
                        DispatchQueue.global(qos: .userInitiated).async {
                            let data = frame.image.pngData()
                            DispatchQueue.main.async {
                                preparingPNG = false
                                if let data { png = data; exporting = true }
                                else { error = "Cannot prepare this image as PNG." }
                            }
                        }
                    }.buttonStyle(PrimaryButton()).disabled(preparingPNG)
                    Button("Export matching diagnostics", systemImage: "square.and.arrow.up") {
                        do {
                            guard let diagnostics = frame.diagnostics else { throw CWFailure(message: "No diagnostics were captured for this frame.") }
                            diagnosticsData = try diagnostics.get()
                            exportingDiagnostics = true
                        } catch { self.error = error.localizedDescription }
                    }.font(.subheadline)
                    if let error { Text(error).foregroundStyle(.red).font(.footnote) }
                }.padding(24)
            }
            .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) {
                Button("Done") { dismiss() }.disabled(preparingPNG)
            } }
            .interactiveDismissDisabled(preparingPNG)
            .sheet(item: $payload) { PayloadView(payload: $0) }
            .fileExporter(isPresented: $exporting, document: BinaryDocument(data: png), contentType: .png,
                          defaultFilename: "\(frame.exportStem).png") { if case .failure(let failure) = $0 { error = failure.localizedDescription } }
            .fileExporter(isPresented: $exportingDiagnostics, document: BinaryDocument(data: diagnosticsData), contentType: .json,
                          defaultFilename: "\(frame.exportStem)-diagnostics.json") { if case .failure(let failure) = $0 { error = failure.localizedDescription } }
        }
    }
}

private enum InspectorMode: String, CaseIterable { case preview = "Preview", text = "Text", hex = "Hex", base64 = "Base64" }

private struct PayloadView: View {
    let payload: DecodedPayload
    var onAppear: () -> Void = {}
    @Environment(\.dismiss) private var dismiss
    @State private var mode = InspectorMode.preview
    @State private var exporting = false
    @State private var exportError: String?
    @State private var copied = false
    private var content: PayloadContent { PayloadContent(data: payload.data) }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    HStack(spacing: 10) {
                        Image(systemName: "checkmark.circle.fill").foregroundStyle(Design.accent)
                        Eyebrow(text: "Recovered intact")
                        Spacer()
                        Text(payload.data.count.formatted() + " bytes").font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                    Text(content.label).font(.system(size: 32, weight: .semibold, design: .serif))
                    if let transfer = payload.transfer {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(String(format: "%.2f Mbit/s", transfer.megabitsPerSecond))
                                .font(.title.weight(.medium).monospacedDigit())
                            Text(String(format: "Verified transfer · %.2f seconds · %d chunks", transfer.elapsedSeconds, transfer.chunks))
                                .font(.subheadline).foregroundStyle(.secondary)
                            Text("Complete-file average, including retries and reassembly. SHA-256 verified.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Picker("View payload", selection: $mode) {
                        ForEach(InspectorMode.allCases, id: \.self) { value in
                            if value != .text || content.text != nil { Text(value.rawValue).tag(value) }
                        }
                    }.pickerStyle(.segmented).onChange(of: mode) { _, _ in copied = false }
                    inspector.frame(maxWidth: .infinity, alignment: .leading)
                    Divider()
                    Button("Export original bytes", systemImage: "square.and.arrow.up") { exporting = true }.buttonStyle(PrimaryButton())
                    Button(copied ? "Copied" : copyTitle, systemImage: copied ? "checkmark" : "doc.on.doc") {
                        UIPasteboard.general.string = mode == .hex ? content.hexadecimal() : (content.text == nil || mode == .base64 ? payload.data.base64EncodedString() : content.text)
                        copied = true
                    }.font(.subheadline)
                    if let exportError { Text(exportError).font(.footnote).foregroundStyle(.red) }
                    DisclosureGroup("Decode details") {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(payload.profileName)
                            if let scanMilliseconds = payload.scanMilliseconds {
                                Text("Scan · \(String(format: "%.1f", scanMilliseconds)) ms")
                            }
                            Text("Final decoder call · \(String(format: "%.1f", payload.finalCallMilliseconds)) ms")
                            Text("\(payload.corrected) corrected locations · \(payload.erased) low-confidence bytes")
                            if let transfer = payload.transfer {
                                Text("\(transfer.duplicates) duplicate chunks · \(transfer.decodeFailures) unsuccessful frame decodes")
                                if transfer.repairSymbols > 0 {
                                    Text("\(transfer.repairedChunks) missing chunks repaired · \(transfer.repairSymbols) repair symbols received")
                                }
                                Text("Correction counts above describe the final chunk.")
                            }
                            Text("Preview formatting never changes exported bytes.")
                        }.font(.caption.monospaced()).padding(.top, 8)
                    }.font(.caption).foregroundStyle(.secondary)
                }.padding(24)
            }
            .background(Design.paper)
            .onAppear(perform: onAppear)
            .navigationTitle("Your payload").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .fileExporter(isPresented: $exporting, document: BinaryDocument(data: payload.data), contentType: content.exportContentType,
                          defaultFilename: "cw-payload.\(content.fileExtension)") {
                if case .failure(let error) = $0 { exportError = error.localizedDescription }
            }
        }
    }
    private var copyTitle: String { mode == .hex ? "Copy hex + ASCII" : (content.text == nil || mode == .base64 ? "Copy Base64" : "Copy text") }
    @ViewBuilder private var inspector: some View {
        switch mode {
        case .preview:
            if let mimeType = content.inertImageMIMEType {
                InertImage(data: payload.data, mimeType: mimeType).frame(height: 280).clipShape(RoundedRectangle(cornerRadius: 12))
                Text(content.isGIF ? "GIF · \(content.gifFrameCount) frames · offline image preview" : "SVG · offline image preview")
                    .font(.caption).foregroundStyle(.secondary)
            } else if let image = content.image {
                Image(uiImage: image).interpolation(.none).resizable().scaledToFit().frame(maxHeight: 340)
                    .padding(20).background(.white, in: RoundedRectangle(cornerRadius: 12))
                Text("\(Int(image.size.width * image.scale)) × \(Int(image.size.height * image.scale)) pixels").font(.caption.monospaced()).foregroundStyle(.secondary)
            } else if let text = content.text { textPreview(text) }
            else {
                Text("Binary payload").font(.title3.weight(.medium))
                Text("This payload is binary. Inspect its hexadecimal or Base64 representation, or export the original file.").foregroundStyle(.secondary)
                codeText(content.hex, limit: 768)
            }
        case .text: textPreview(content.text ?? "")
        case .hex:
            ScrollView(.horizontal) { codeText(content.hex, limit: Int.max) }
            if payload.data.count > 8192 { Text("Hex preview: first 8 KiB. Export contains every byte.").font(.caption).foregroundStyle(.secondary) }
        case .base64: codeText(payload.data.base64EncodedString(), limit: 16384)
        }
    }
    private func textPreview(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(text.isEmpty ? "Empty text" : String(text.prefix(16384))).font(.body).lineSpacing(5).textSelection(.enabled)
            if text.count > 16384 { Text("Preview limited to 16,384 characters.").font(.caption).foregroundStyle(.secondary) }
        }.padding(20).frame(maxWidth: .infinity, alignment: .leading).background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12))
    }
    private func codeText(_ text: String, limit: Int = 32768) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(String(text.prefix(limit))).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
            if text.count > limit { Text("Preview truncated. Copy or export for the complete data.").font(.caption).foregroundStyle(.secondary) }
        }
    }
}

private struct InertImage: UIViewRepresentable {
    let data: Data
    let mimeType: String
    final class Coordinator: NSObject, WKNavigationDelegate {
        var loadedData: Data?
        var loadedMIMEType: String?
        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            decisionHandler(navigationAction.request.url?.absoluteString == "about:blank" ? .allow : .cancel)
        }
    }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.isScrollEnabled = false
        return view
    }
    func updateUIView(_ view: WKWebView, context: Context) {
        guard context.coordinator.loadedData != data || context.coordinator.loadedMIMEType != mimeType else { return }
        context.coordinator.loadedData = data
        context.coordinator.loadedMIMEType = mimeType
        let encoded = data.base64EncodedString()
        view.loadHTMLString("""
        <!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"><style>html,body{margin:0;width:100%;height:100%;overflow:hidden}img{width:100%;height:100%;object-fit:contain}</style></head><body><img alt="Recovered image" src="data:\(mimeType);base64,\(encoded)"></body></html>
        """, baseURL: nil)
    }
}

private enum ErrorCorrection: String, CaseIterable { case standard = "Standard", low = "Low" }

private struct CreateView: View {
    @State private var text = "Hello from ChromaWeave."
    @State private var binaryPayload: Data?
    @State private var filename: String?
    @State private var errorCorrection = ErrorCorrection.standard
    @State private var cellPixels = 4
    @State private var losslessCompression = false
    @State private var encoded: EncodedCode?
    @State private var busy = false
    @State private var error: String?
    @State private var exporting = false
    @State private var importing = false
    private var payload: Data { binaryPayload ?? Data(text.utf8) }
    private var encodingProfile: Int { errorCorrection == .low ? 4 : Codec.defaultProfile }
    var body: some View {
        NavigationStack {
            ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    VStack(alignment: .leading, spacing: 8) {
                        Eyebrow(text: "Text, images and files")
                        Text("Create CW Code").font(.system(size: 34, weight: .semibold, design: .serif))
                    }
                    HStack {
                        Eyebrow(text: filename ?? "Your message")
                        Spacer()
                        Text(payload.count.formatted() + " bytes").font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                    if binaryPayload != nil {
                        Label(PayloadContent(data: payload).label, systemImage: "doc").font(.headline)
                        Button("Write text instead") { binaryPayload = nil; filename = nil; encoded = nil }.font(.subheadline)
                    } else {
                        TextEditor(text: $text).font(.body).frame(minHeight: 116).padding(12)
                            .scrollContentBackground(.hidden).background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12))
                    }
                    HStack {
                        Button("Import file", systemImage: "folder") { importing = true }
                        Spacer()
                        Menu {
                            ForEach(CodeSample.allCases) { sample in
                                Button(sample.title) { binaryPayload = sample.data; filename = sample.title; encoded = nil }
                            }
                        } label: { Label("Samples", systemImage: "square.stack") }
                    }.font(.subheadline)
                    Divider()
                    VStack(alignment: .leading, spacing: 8) {
                        Text("CW Luma10 error correction").font(.subheadline)
                        Picker("Error correction", selection: $errorCorrection) {
                            ForEach(ErrorCorrection.allCases, id: \.self) { value in
                                Text(value.rawValue).tag(value)
                            }
                        }.pickerStyle(.segmented)
                        Text(errorCorrection == .standard
                             ? "More protection against scan errors."
                             : "Less protection; can reduce code size.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Toggle("Lossless compression", isOn: $losslessCompression)
                        .font(.subheadline)
                    Text("Compress when it saves space; decoding restores the original bytes.")
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        Text("Render size").font(.subheadline)
                        Spacer()
                        Picker("Render size", selection: $cellPixels) {
                            Text("Compact").tag(4)
                            Text("Medium").tag(6)
                            Text("Large").tag(8)
                        }.pickerStyle(.menu).labelsHidden()
                    }
                    Text("CW Luma10 uses a compact border. Render size controls the exported pixels; resize the display or print for scanning.")
                        .font(.caption).foregroundStyle(.secondary)
                    Button { generate() } label: {
                        HStack { if busy { ProgressView().tint(.white) }; Text(busy ? "Encoding…" : "Create code"); Image(systemName: "arrow.right") }
                    }.buttonStyle(PrimaryButton())
                    if let error { Label(error, systemImage: "exclamationmark.circle").font(.footnote).foregroundStyle(.red) }
                    if let encoded {
                        VStack(alignment: .leading, spacing: 16) {
                            Image(uiImage: encoded.image).interpolation(.none).resizable().scaledToFit().padding(12).background(.white)
                            Text("CW Luma10 · \(errorCorrection.rawValue) error correction")
                                .font(.caption).foregroundStyle(.secondary)
                            if encoded.compression != "none" {
                                Text("Original \(encoded.bytes.formatted()) bytes · stored \(encoded.storedBytes.formatted()) bytes")
                                    .font(.caption.monospaced()).foregroundStyle(.secondary)
                            }
                            HStack {
                                Text("\(encoded.side) × \(encoded.side) px").font(.caption.monospaced()).foregroundStyle(.secondary)
                                Spacer()
                                Button("Export PNG", systemImage: "square.and.arrow.up") { exporting = true }.font(.subheadline.weight(.semibold))
                            }
                            Text("Show the full border on one device. Scan it with another.").font(.footnote).foregroundStyle(.secondary)
                        }.id("generated-code")
                    }
                }.padding(24)
            }
            .onChange(of: busy) { _, inProgress in
                if !inProgress && encoded != nil { withAnimation { scroll.scrollTo("generated-code", anchor: .top) } }
            }
            .background(Design.paper).disabled(busy)
            .navigationTitle("Create").navigationBarTitleDisplayMode(.inline)
            .onChange(of: errorCorrection) { _, _ in encoded = nil; error = nil }
            .onChange(of: cellPixels) { _, _ in encoded = nil }
            .onChange(of: losslessCompression) { _, _ in encoded = nil; error = nil }
            .onChange(of: text) { _, _ in encoded = nil }
            .fileExporter(isPresented: $exporting, document: BinaryDocument(data: encoded?.png ?? Data()), contentType: .png,
                          defaultFilename: "cw-code.png") { if case .failure(let failure) = $0 { error = failure.localizedDescription } }
            .fileImporter(isPresented: $importing, allowedContentTypes: [.data]) { result in
                do {
                    let url = try result.get()
                    let access = url.startAccessingSecurityScopedResource()
                    defer { if access { url.stopAccessingSecurityScopedResource() } }
                    let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                    guard let maximum = Codec.maximumInputBytes(profile: encodingProfile, losslessCompression: losslessCompression) else {
                        throw CWFailure(message: "Unknown CW Luma10 profile.")
                    }
                    guard size <= maximum else {
                        let kind = losslessCompression ? "Compression accepts files" : "This profile accepts files"
                        throw CWFailure(message: "\(kind) up to \(maximum.formatted()) bytes.")
                    }
                    binaryPayload = try Data(contentsOf: url)
                    filename = url.lastPathComponent
                    encoded = nil
                    error = nil
                } catch { self.error = error.localizedDescription }
            }
            }
        }
    }
    private func generate() {
        guard let maximum = Codec.maximumInputBytes(profile: encodingProfile, losslessCompression: losslessCompression) else {
            error = "Unknown CW Luma10 profile."
            return
        }
        guard payload.count <= maximum else {
            let kind = losslessCompression ? "Compression accepts payloads" : "This profile accepts payloads"
            error = "\(kind) up to \(maximum.formatted()) bytes."
            return
        }
        busy = true; error = nil; encoded = nil
        let data = payload, selectedProfile = encodingProfile, pitch = cellPixels
        let compress = losslessCompression
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let code = try Codec.encode(data, profile: selectedProfile, cellPixels: pitch,
                                            losslessCompression: compress)
                DispatchQueue.main.async { encoded = code; busy = false }
            } catch { DispatchQueue.main.async { self.error = error.localizedDescription; busy = false } }
        }
    }
}
