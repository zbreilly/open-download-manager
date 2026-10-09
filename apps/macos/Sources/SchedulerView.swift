import SwiftUI

struct SchedulerView: View {
    @EnvironmentObject var store: AppStore
    @State private var selectedId: String? = "main"
    @State private var draft: DownloadQueue?
    @State private var dirty = false
    /// The user reordered the list: only then is the order sent.
    @State private var reordered = false
    @State private var saving = false
    @State private var saveError = ""
    @State private var newName = ""
    @State private var naming = false

    private static let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]

    var body: some View {
        NavigationSplitView {
            List(selection: $selectedId) {
                ForEach(store.queues) { q in
                    HStack {
                        Label(q.name, systemImage: q.running ? "play.circle.fill" : (q.builtin ? "list.bullet" : "clock"))
                        Spacer()
                        Text("\(q.order.count)").monospacedDigit().foregroundStyle(.secondary)
                    }
                    .tag(q.id)
                }
            }
            .safeAreaInset(edge: .bottom) {
                Button { naming = true } label: { Label("New Queue", systemImage: "plus") }
                    .buttonStyle(.borderless)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .navigationSplitViewColumnWidth(min: 170, ideal: 210)
        } detail: {
            if draft != nil {
                editor
            } else {
                Text("Select a queue").foregroundStyle(.secondary)
            }
        }
        .onAppear { pick(selectedId) }
        .onChange(of: selectedId) { id in pick(id) }
        .onChange(of: store.queues) { qs in
            // a queue created here shows up once the engine reports it
            if draft == nil, let id = selectedId, qs.contains(where: { $0.id == id }) {
                pick(id)
                return
            }
            guard let d = draft, let q = qs.first(where: { $0.id == d.id }) else { return }
            draft?.running = q.running
            if !reordered { draft?.order = q.order }
        }
        .alert("New Queue", isPresented: $naming) {
            TextField("Name", text: $newName)
            Button("Create") { createQueue() }
            Button("Cancel", role: .cancel) { newName = "" }
        }
    }

    private func pick(_ id: String?) {
        draft = store.queues.first(where: { $0.id == id })
        dirty = false
        reordered = false
        saveError = ""
    }

    private var binding: Binding<DownloadQueue> {
        Binding(get: { draft ?? DownloadQueue.new(name: "") }, set: { draft = $0; dirty = true })
    }

    private var editor: some View {
        let q = binding
        return VStack(spacing: 0) {
            Form {
                Section {
                    HStack {
                        if q.wrappedValue.builtin {
                            Text(q.wrappedValue.name).font(.title3.weight(.semibold))
                        } else {
                            TextField("Name", text: q.name).font(.title3.weight(.semibold))
                        }
                        if q.wrappedValue.running {
                            Label("Running", systemImage: "circle.fill").foregroundStyle(.green).font(.caption)
                        }
                        Spacer()
                        Button("Start Now") { store.startQueue(q.wrappedValue.id) }.disabled(q.wrappedValue.running)
                        Button("Stop") { store.stopQueue(q.wrappedValue.id) }.disabled(!q.wrappedValue.running)
                    }
                }
                Section("Schedule") {
                    HStack {
                        Toggle("Start at", isOn: q.startEnabled)
                        Spacer()
                        DatePicker("", selection: timeBinding(q.startTime), displayedComponents: .hourAndMinute)
                            .labelsHidden().disabled(!q.wrappedValue.startEnabled)
                    }
                    HStack {
                        Toggle("Stop at", isOn: q.stopEnabled)
                        Spacer()
                        DatePicker("", selection: timeBinding(q.stopTime), displayedComponents: .hourAndMinute)
                            .labelsHidden().disabled(!q.wrappedValue.stopEnabled)
                    }
                    LabeledContent("Repeat on") {
                        HStack(spacing: 4) {
                            ForEach(0..<7, id: \.self) { i in
                                Toggle(Self.days[i], isOn: Binding(
                                    get: { q.wrappedValue.days.indices.contains(i) ? q.wrappedValue.days[i] : true },
                                    set: { v in
                                        var dd = q.wrappedValue.days
                                        if dd.count != 7 { dd = Array(repeating: true, count: 7) }
                                        dd[i] = v
                                        q.wrappedValue.days = dd
                                    }))
                                .toggleStyle(.button)
                            }
                        }
                    }
                    if let next = nextRun(q.wrappedValue) {
                        LabeledContent("Next run", value: next)
                    }
                }
                Section("Behavior") {
                    Stepper(value: Binding(get: { Int(q.wrappedValue.maxConcurrent) }, set: { q.wrappedValue.maxConcurrent = UInt32($0) }), in: 1...32) {
                        LabeledContent("Files at the same time", value: "\(q.wrappedValue.maxConcurrent)")
                    }
                    Picker("Retry failed downloads", selection: q.retryFailed) {
                        Text("Never").tag(UInt32(0))
                        Text("Once").tag(UInt32(1))
                        Text("3 times").tag(UInt32(3))
                        Text("5 times").tag(UInt32(5))
                        Text("10 times").tag(UInt32(10))
                    }
                    Picker("When the queue finishes", selection: q.onFinish) {
                        Text("Do nothing").tag("none")
                        Text("Put the Mac to sleep").tag("sleep")
                        Text("Shut down the Mac").tag("shutdown")
                        Text("Quit Open Download Manager").tag("exit")
                    }
                    if q.wrappedValue.onFinish == "sleep" || q.wrappedValue.onFinish == "shutdown" {
                        Text("A 60-second countdown with a Cancel button appears first.").font(.caption).foregroundStyle(.secondary)
                    }
                }
                Section("Order") {
                    if q.wrappedValue.order.isEmpty {
                        Text("No files in this queue. Use “Move to Queue” on a download, or pick this queue when adding.")
                            .foregroundStyle(.secondary)
                    } else {
                        List {
                            ForEach(q.wrappedValue.order, id: \.self) { id in
                                HStack {
                                    if let d = store.download(id) {
                                        FileIconView(fileExtension: d.fileExtension, size: 16)
                                        Text(d.fileName).lineLimit(1).truncationMode(.middle)
                                        Spacer()
                                        Text(Fmt.statusLabel(d)).foregroundStyle(.secondary).font(.caption)
                                        Text(Fmt.bytes(d.totalBytes)).monospacedDigit().foregroundStyle(.secondary)
                                    } else {
                                        Text(id).foregroundStyle(.secondary)
                                    }
                                }
                            }
                            .onMove { from, to in
                                q.wrappedValue.order.move(fromOffsets: from, toOffset: to)
                                reordered = true
                            }
                        }
                        .frame(minHeight: 140)
                        Text("Drag to reorder.").font(.caption).foregroundStyle(.secondary)
                    }
                }
                if !q.wrappedValue.builtin {
                    Section {
                        Button("Delete Queue", role: .destructive) {
                            store.act("delete_queue", ["id": q.wrappedValue.id])
                            selectedId = "main"
                        }
                    }
                }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                if saveError.isEmpty {
                    Text(summary(q.wrappedValue)).font(.caption).foregroundStyle(.secondary)
                } else {
                    Text(saveError).font(.caption).foregroundStyle(.red).textSelection(.enabled)
                        .lineLimit(2)
                }
                Spacer()
                Button("Revert") { pick(selectedId) }.disabled(!dirty || saving)
                Button("Save Schedule") { save() }.keyboardShortcut(.defaultAction).disabled(!dirty || saving)
            }
            .padding(12)
        }
    }

    private func timeBinding(_ t: Binding<String>) -> Binding<Date> {
        Binding(get: {
            let parts = t.wrappedValue.split(separator: ":").compactMap { Int($0) }
            var c = DateComponents()
            c.hour = parts.first ?? 0
            c.minute = parts.count > 1 ? parts[1] : 0
            return Calendar.current.date(from: c) ?? Date()
        }, set: { d in
            let c = Calendar.current.dateComponents([.hour, .minute], from: d)
            t.wrappedValue = String(format: "%02d:%02d", c.hour ?? 0, c.minute ?? 0)
        })
    }

    private func nextRun(_ q: DownloadQueue) -> String? {
        guard q.startEnabled else { return nil }
        let parts = q.startTime.split(separator: ":").compactMap { Int($0) }
        guard parts.count == 2 else { return nil }
        let cal = Calendar.current
        let now = Date()
        for add in 0..<8 {
            guard let day = cal.date(byAdding: .day, value: add, to: now),
                  let t = cal.date(bySettingHour: parts[0], minute: parts[1], second: 0, of: day) else { continue }
            let wd = (cal.component(.weekday, from: t) + 5) % 7 // Mon = 0
            if t > now && (q.days.indices.contains(wd) ? q.days[wd] : true) {
                let f = DateFormatter()
                f.dateFormat = add == 0 ? "'today' HH:mm" : add == 1 ? "'tomorrow' HH:mm" : "EEEE HH:mm"
                return f.string(from: t)
            }
        }
        return nil
    }

    private func summary(_ q: DownloadQueue) -> String {
        let pending = q.order.compactMap { store.download($0) }.filter { $0.status != .complete }
        let left = pending.reduce(UInt64(0)) { $0 + (($1.totalBytes ?? 0) > $1.receivedBytes ? ($1.totalBytes ?? 0) - $1.receivedBytes : 0) }
        return "Next run: \(nextRun(q) ?? "not scheduled") · \(pending.count) files · \(Fmt.bytes(left))"
    }

    /// The user's arrangement applied to the live membership: downloads that
    /// left the queue are dropped, ones that joined since go to the end.
    private func liveOrder(_ wanted: [String], id: String) -> [String] {
        let live = store.queues.first(where: { $0.id == id })?.order ?? wanted
        let inLive = Set(live)
        let kept = wanted.filter { inLive.contains($0) }
        let keptSet = Set(kept)
        return kept + live.filter { !keptSet.contains($0) }
    }

    private func save() {
        guard var d = draft, !saving else { return }
        d.maxConcurrent = min(32, max(1, d.maxConcurrent))
        let wasReordered = reordered
        let wanted = d.order
        saving = true
        saveError = ""
        Task { @MainActor in
            do {
                // settings only: the engine keeps the queue's current order
                let saved = try await store.saveQueue(d)
                if wasReordered {
                    try await Engine.shared.runAwait("reorder_queue", ["id": saved.id, "order": liveOrder(wanted, id: saved.id)])
                }
                dirty = false
                reordered = false
            } catch {
                // keep the edits (dirty) so the user can retry
                saveError = "Could not save: \(error.localizedDescription)"
            }
            saving = false
        }
    }

    private func createQueue() {
        let name = newName.trimmingCharacters(in: .whitespaces)
        newName = ""
        guard !name.isEmpty else { return }
        Task { @MainActor in
            do {
                let saved = try await store.saveQueue(DownloadQueue.new(name: name))
                selectedId = saved.id
            } catch {
                store.report("Could not create the queue: \(error.localizedDescription)")
            }
        }
    }
}
