import Foundation

/// Thin Swift wrapper over the Rust engine's C interface (odm.h).
/// Every call is method name + JSON arguments → JSON result.
enum EngineError: LocalizedError {
    case message(String)
    var errorDescription: String? {
        switch self {
        case .message(let m): return m
        }
    }
}

/// Carries JSON-compatible arguments across threads.
final class ArgsBox: @unchecked Sendable {
    let args: [String: Any]
    init(_ a: [String: Any]) { args = a }
}

final class Engine: @unchecked Sendable {
    static let shared = Engine()
    private init() {}

    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()

    /// Called on an engine thread for every engine event.
    var onEvent: (@Sendable (String, Data) -> Void)?

    func start(dataDir: URL) throws {
        let cb: odm_event_cb = { namePtr, jsonPtr, _ in
            guard let namePtr = namePtr, let jsonPtr = jsonPtr else { return }
            let name = String(cString: namePtr)
            let data = Data(bytes: jsonPtr, count: strlen(jsonPtr))
            Engine.shared.onEvent?(name, data)
        }
        let rc = dataDir.path.withCString { odm_start($0, cb, nil) }
        if rc != 0 {
            let msg = String(cString: odm_last_error())
            throw EngineError.message(msg.isEmpty ? "engine failed to start (\(rc))" : msg)
        }
    }

    func shutdown() { odm_shutdown() }

    /// Encode an Encodable into a JSON-compatible value for argument dictionaries.
    func jsonValue<T: Encodable>(_ v: T) -> Any {
        guard let d = try? encoder.encode(v),
              let o = try? JSONSerialization.jsonObject(with: d, options: [.fragmentsAllowed]) else { return NSNull() }
        return o
    }

    private func raw(_ method: String, _ args: [String: Any]) throws -> Data {
        let argData = try JSONSerialization.data(withJSONObject: args, options: [])
        let argString = String(data: argData, encoding: .utf8) ?? "{}"
        guard let out = method.withCString({ m in argString.withCString { a in odm_call(m, a) } }) else {
            throw EngineError.message("engine returned nothing")
        }
        defer { odm_free(out) }
        return Data(bytes: out, count: strlen(out))
    }

    private struct Envelope<T: Decodable>: Decodable {
        var ok: T?
        var err: String?
    }

    private struct ErrOnly: Decodable { var err: String? }

    /// Call and decode the result.
    func call<T: Decodable>(_ method: String, _ args: [String: Any] = [:], as: T.Type = T.self) throws -> T {
        let data = try raw(method, args)
        let env: Envelope<T>
        do {
            env = try decoder.decode(Envelope<T>.self, from: data)
        } catch {
            if let e = try? decoder.decode(ErrOnly.self, from: data), let msg = e.err {
                throw EngineError.message(msg)
            }
            throw EngineError.message("could not read engine reply for \(method): \(error)")
        }
        if let e = env.err { throw EngineError.message(e) }
        guard let v = env.ok else { throw EngineError.message("empty reply for \(method)") }
        return v
    }

    /// Call when only success/failure matters.
    func run(_ method: String, _ args: [String: Any] = [:]) throws {
        let data = try raw(method, args)
        if let e = try? decoder.decode(ErrOnly.self, from: data), let msg = e.err {
            throw EngineError.message(msg)
        }
    }

    /// Fire-and-forget on a background queue; errors go to the callback on main.
    func runAsync(_ method: String, _ args: [String: Any] = [:], onError: (@MainActor (String) -> Void)? = nil) {
        let box = ArgsBox(args)
        DispatchQueue.global(qos: .userInitiated).async {
            do { try self.run(method, box.args) } catch {
                let msg = error.localizedDescription
                if let onError = onError {
                    Task { @MainActor in onError(msg) }
                }
            }
        }
    }

    /// Async/await variant for calls that may block (probe, grab, refresh).
    func callAsync<T: Decodable>(_ method: String, _ args: [String: Any] = [:], as: T.Type = T.self) async throws -> T {
        let box = ArgsBox(args)
        return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<T, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do { cont.resume(returning: try self.call(method, box.args, as: T.self)) } catch { cont.resume(throwing: error) }
            }
        }
    }

    func runAwait(_ method: String, _ args: [String: Any] = [:]) async throws {
        let box = ArgsBox(args)
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            DispatchQueue.global(qos: .userInitiated).async {
                do { try self.run(method, box.args); cont.resume() } catch { cont.resume(throwing: error) }
            }
        }
    }

    func decode<T: Decodable>(_ type: T.Type, _ data: Data) -> T? {
        try? decoder.decode(T.self, from: data)
    }
}
