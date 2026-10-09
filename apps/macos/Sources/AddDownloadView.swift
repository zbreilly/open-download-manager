import AppKit
import SwiftUI

struct AddDownloadView: View {
    @EnvironmentObject var store: AppStore
    var params: AddParams

    @State private var url = ""
    @State private var batch = false
    @State private var batchURLs: [String] = []
    @State private var probing = false
    @State private var probe: ProbeResult?
    @State private var probeError = ""
    @State private var fileName = ""
    @State private var autoName = ""
    private var nameTouched: Bool { fileName != autoName }
    @State private var category = "general"
    @State private var autoCategory = "general"
    @State private var categoryTouched = false
    @State private var alwaysCategory = false
    @State private var queueId = ""
    @State private var saveDir = ""
    @State private var autoDir = ""
    private var dirTouched: Bool { saveDir != autoDir }
    @State private var connections: UInt32 = 16
    @State private var remember = true
    @State private var verify = false
    @State private var checksum = ""
    @State private var showAdvanced = false
    @State private var username = ""
    @State private var password = ""
    @State private var referrer = ""
    @State private var cookies = ""
    @State private var userAgent = ""
    @State private var headers: [HeaderPair] = []
    @State private var recents: [String] = []
    @State private var askExists = false
    @State private var pendingStart = true
    @State private var busy = false
    @State private var submitError = ""
    @State private var probeTask: Task<Void, Never>?
    @State private var loaded = false

    struct HeaderPair: Identifiable {
        let id = UUID()
        var key = ""
        var value = ""
    }

    private var isURL: Bool { Self.isHTTP(url) }
    private var valid: Bool { batch ? !batchURLs.isEmpty : isURL }
    private var resumable: Bool { probe?.resumable ?? true }
    private var nameExt: String {
        let e = (fileName as NSString).pathExtension.lowercased()
        return e.count <= 10 ? e : ""
    }
    private var categoryName: String {
        store.settings?.categories.first(where: { $0.id == category })?.name ?? "this category"
    }

    static func isHTTP(_ s: String) -> Bool {
        guard let u = URL(string: s.trimmingCharacters(in: .whitespacesAndNewlines)),
              let sc = u.scheme?.lowercased(), sc == "http" || sc == "https", u.host != nil else { return false }
        return true
    }

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    if batch {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Links, one per line. Patterns like file[001-120].jpg or [a-z] expand.")
                                .font(.caption).foregroundStyle(.secondary)
                            TextEditor(text: $url)
                                .font(.system(.body, design: .monospaced))
                                .frame(minHeight: 90)
                        }
                        LabeledContent("Files") {
                            Text("\(batchURLs.count)").monospacedDigit()
                        }
                    } else {
                        TextField("URL", text: $url, prompt: Text("https://"))
                            .font(.system(.body, design: .monospaced))
                        if !url.isEmpty && !isURL {
                            Text("Enter a full http:// or https:// address.").foregroundStyle(.red).font(.caption)
                        }
                        if isURL { infoRow }
                    }
                }

                Section {
                    if !batch {
                        TextField("File name", text: $fileName)
                        Picker("Category", selection: Binding(get: { category }, set: { setCategory($0, touched: true) })) {
                            ForEach(store.settings?.categories ?? []) { c in Text(c.name).tag(c.id) }
                        }
                    }
                    Picker("Queue", selection: $queueId) {
                        Text("None (start manually)").tag("")
                        ForEach(store.queues) { q in Text(q.name).tag(q.id) }
                    }
                    LabeledContent("Save to") {
                        HStack {
                            TextField("Folder", text: $saveDir)
                                .labelsHidden()
                                .font(.system(.body, design: .monospaced))
                            Menu {
                                ForEach(recents, id: \.self) { r in
                                    Button(r) { saveDir = r }
                                }
                                if !recents.isEmpty { Divider() }
                                Button("Choose…") { browse() }
                            } label: {
                                Text("Choose…")
                            } primaryAction: {
                                browse()
                            }
                            .fixedSize()
                        }
                    }
                }

                Section {
                    Picker("Connections", selection: $connections) {
                        ForEach([UInt32(1), 4, 8, 16, 32], id: \.self) { n in Text("\(n)").tag(n) }
                    }
                    .pickerStyle(.segmented)
                    .disabled(!resumable)
                    if !batch {
                        if !nameExt.isEmpty && category != autoCategory {
                            Toggle("Always put .\(nameExt) files in \(categoryName)", isOn: $alwaysCategory)
                        }
                        Toggle("Remember this folder for \(categoryName)", isOn: $remember)
                        Toggle("Verify checksum after download", isOn: $verify)
                        if verify {
                            TextField("Checksum", text: $checksum, prompt: Text("SHA-256, SHA-1 or MD5"))
                                .font(.system(.body, design: .monospaced))
                        }
                    }
                }

                Section {
                    DisclosureGroup("Authentication, referrer, cookies, headers", isExpanded: $showAdvanced) {
                        TextField("Username", text: $username)
                        SecureField("Password", text: $password)
                        TextField("Referrer", text: $referrer)
                        TextField("Cookies", text: $cookies, prompt: Text("name=value; other=value"))
                        TextField("User agent", text: $userAgent, prompt: Text("Default"))
                        ForEach($headers) { $h in
                            HStack {
                                TextField("Header", text: $h.key).labelsHidden()
                                TextField("Value", text: $h.value).labelsHidden()
                                Button { headers.removeAll { $0.id == h.id } } label: { Image(systemName: "minus.circle") }
                                    .buttonStyle(.borderless)
                            }
                        }
                        Button("Add Header") { headers.append(HeaderPair()) }
                    }
                }

                if !submitError.isEmpty {
                    Section { Text(submitError).foregroundStyle(.red).textSelection(.enabled) }
                }
            }
            .formStyle(.grouped)

            Divider()
            HStack {
                Toggle("Batch", isOn: $batch).toggleStyle(.switch).help("Add several links or a numbered pattern")
                Spacer()
                Button("Add to Queue") { submit(start: false) }.disabled(!valid || busy)
                Button("Cancel") { closeKeyWindow() }.keyboardShortcut(.cancelAction)
                Button(batch ? "Start \(batchURLs.count) Downloads" : "Start Download") { submit(start: true) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!valid || busy)
            }
            .padding(14)
        }
        .frame(minWidth: 480, minHeight: 520)
        .confirmationDialog("“\(fileName)” already exists in this folder.", isPresented: $askExists, titleVisibility: .visible) {
            Button("Rename New File") { submit(start: pendingStart, onExists: "rename") }
            Button("Resume Existing File") { submit(start: pendingStart, onExists: "resume") }
            Button("Overwrite", role: .destructive) { submit(start: pendingStart, onExists: "overwrite") }
            Button("Cancel", role: .cancel) {}
        }
        .onChange(of: url) { _ in scheduleProbe() }
        .onChange(of: batch) { _ in scheduleProbe() }
        .onAppear(perform: setup)
    }

    private var infoRow: some View {
        HStack(spacing: 12) {
            FileIconView(fileExtension: nameExt, size: 36)
            VStack(alignment: .leading, spacing: 3) {
                if probing {
                    ProgressView().controlSize(.small)
                    Text("Checking link…").font(.caption).foregroundStyle(.secondary)
                } else if !probeError.isEmpty {
                    Text(probeError).foregroundStyle(.red).font(.callout)
                    Text("You can still add it; it will retry when started.").font(.caption).foregroundStyle(.secondary)
                } else if let p = probe {
                    Text("\(Fmt.bytes(p.totalBytes))\(p.mime.map { " · \($0)" } ?? "")")
                        .font(.system(.callout, design: .monospaced))
                    if p.resumable {
                        HStack(spacing: 12) {
                            Label("Resumable", systemImage: "checkmark").foregroundStyle(.green)
                            Label("Byte ranges", systemImage: "checkmark").foregroundStyle(.green)
                            Text(p.protocol).foregroundStyle(.secondary)
                        }
                        .font(.caption)
                    } else {
                        Label("Server doesn't support resume. Download will use 1 connection.", systemImage: "exclamationmark.triangle")
                            .font(.caption).foregroundStyle(.orange)
                    }
                }
            }
            Spacer()
        }
    }

    // MARK: logic

    private func setup() {
        guard !loaded else { return }
        connections = store.settings?.defaultConnections ?? 16
        recents = (try? Engine.shared.call("recent_folders", ["limit": 6])) ?? []
        saveDir = (try? Engine.shared.call("folder_for", ["category": "general"])) ?? ""
        autoDir = saveDir
        batch = params.batch
        if !params.url.isEmpty {
            url = params.url
        } else if !batch, let clip = NSPasteboard.general.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines),
                  Self.isHTTP(clip), !clip.contains(where: { $0.isWhitespace }) {
            url = clip
        }
        if let n = params.fileName, !n.isEmpty { fileName = n }
        referrer = params.referrer ?? ""
        cookies = params.cookies ?? ""
        userAgent = params.userAgent ?? ""
        loaded = true
        scheduleProbe()
    }

    private func setCategory(_ c: String, touched: Bool) {
        category = c
        if touched { categoryTouched = true }
        if !dirTouched {
            saveDir = (try? Engine.shared.call("folder_for", ["category": c])) ?? saveDir
            autoDir = saveDir
        }
    }

    private func browse() {
        if let p = chooseFolder(start: saveDir) {
            saveDir = p
        }
    }

    private func scheduleProbe() {
        probeTask?.cancel()
        let u = url.trimmingCharacters(in: .whitespacesAndNewlines)
        if batch {
            probeTask = Task {
                try? await Task.sleep(nanoseconds: 300_000_000)
                if Task.isCancelled { return }
                var out: [String] = []
                for line in u.split(whereSeparator: { $0.isWhitespace }).map(String.init) {
                    if line.range(of: #"\[[^\]]+-[^\]]+\]"#, options: .regularExpression) != nil {
                        out += (try? await Engine.shared.callAsync("expand_batch", ["pattern": line], as: [String].self)) ?? []
                    } else {
                        out.append(line)
                    }
                }
                batchURLs = Array(out.filter(Self.isHTTP).prefix(10_000))
            }
            return
        }
        probe = nil
        probeError = ""
        guard Self.isHTTP(u) else { probing = false; return }
        probing = true
        let opts: [String: Any] = [
            "referrer": orNull(referrer),
            "cookies": orNull(cookies),
            "username": orNull(username),
            "password": orNull(password),
            "headers": headers.filter { !$0.key.isEmpty }.map { [$0.key, $0.value] },
        ]
        probeTask = Task {
            try? await Task.sleep(nanoseconds: 300_000_000)
            if Task.isCancelled { return }
            do {
                let p: ProbeResult = try await Engine.shared.callAsync("probe_url", ["url": u, "opts": opts])
                if Task.isCancelled { return }
                probe = p
                if !nameTouched { fileName = p.fileName; autoName = p.fileName }
                autoCategory = p.category
                if !categoryTouched { setCategory(p.category, touched: false) }
                if !p.resumable { connections = 1 }
                else if connections == 1 { connections = store.settings?.defaultConnections ?? 16 }
            } catch {
                if Task.isCancelled { return }
                probeError = error.localizedDescription
                if !nameTouched, let last = URL(string: u)?.lastPathComponent, !last.isEmpty, last != "/" {
                    fileName = last.removingPercentEncoding ?? last
                    autoName = fileName
                }
            }
            probing = false
        }
    }

    private func request(start: Bool, link: String, onExists: String?) -> AddRequest {
        AddRequest(
            url: link,
            fileName: batch ? nil : (fileName.trimmingCharacters(in: .whitespaces).isEmpty ? nil : fileName),
            saveDir: dirTouched ? saveDir : nil,
            category: batch ? nil : category,
            queueId: queueId.isEmpty ? (start ? nil : "main") : queueId,
            start: start,
            connections: connections,
            referrer: referrer.isEmpty ? nil : referrer,
            cookies: cookies.isEmpty ? nil : cookies,
            userAgent: userAgent.isEmpty ? nil : userAgent,
            headers: headers.filter { !$0.key.isEmpty }.map { [$0.key, $0.value] },
            username: username.isEmpty ? nil : username,
            password: password.isEmpty ? nil : password,
            checksum: (!batch && verify && !checksum.isEmpty) ? Checksum(algo: "", expected: checksum.trimmingCharacters(in: .whitespaces)) : nil,
            onExists: onExists ?? "rename",
            rememberFolder: !batch && remember && dirTouched,
            totalBytes: batch ? nil : probe?.totalBytes,
            resumable: batch ? nil : probe?.resumable,
            mime: batch ? nil : probe?.mime
        )
    }

    private func submit(start: Bool, onExists: String? = nil) {
        guard valid, !busy else { return }
        submitError = ""
        let dir = dirTouched ? saveDir : ((try? Engine.shared.call("folder_for", ["category": category], as: String.self)) ?? saveDir)
        if !batch && onExists == nil && !fileName.isEmpty {
            let exists: Bool = (try? Engine.shared.call("target_exists", ["dir": dir, "name": fileName])) ?? false
            if exists {
                pendingStart = start
                askExists = true
                return
            }
        }
        busy = true
        let e = Engine.shared
        let reqs: [AddRequest] = batch
            ? batchURLs.map { request(start: start, link: $0, onExists: nil) }
            : [request(start: start, link: url.trimmingCharacters(in: .whitespacesAndNewlines), onExists: onExists)]
        let assign = !batch && alwaysCategory && !nameExt.isEmpty && category != autoCategory
        let ext = nameExt, cat = category
        let showProgress = start && (store.settings?.showProgressOnStart ?? false)
        Task {
            do {
                if assign {
                    try await e.runAwait("assign_extensions", ["category": cat, "extensions": [ext]])
                }
                if reqs.count == 1 {
                    let id: String = try await e.callAsync("add_download", ["req": e.jsonValue(reqs[0])])
                    if showProgress { store.openProgress(id) }
                } else {
                    let _: [String] = try await e.callAsync("add_many", ["reqs": e.jsonValue(reqs)])
                }
                closeKeyWindow()
            } catch {
                submitError = error.localizedDescription
                busy = false
            }
        }
    }
}
