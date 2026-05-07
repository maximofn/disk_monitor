import AppKit
import Foundation
import OSLog

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let config: Config
    private let logger = Logger(subsystem: "com.maximofn.disk-monitor", category: "app")
    private var controller: StatusBarController?
    private var client: SSEClient?
    private var consumerTask: Task<Void, Never>?

    nonisolated init(config: Config) {
        self.config = config
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        let renderer = IconRenderer(height: config.iconHeight)
        let controller = StatusBarController(renderer: renderer, backendURL: config.backendURL)
        self.controller = controller

        let (stream, continuation) = AsyncStream<ClientUpdate>.makeStream(
            bufferingPolicy: .bufferingNewest(8)
        )

        let client = SSEClient(backendURL: config.backendURL)
        self.client = client
        Task {
            await client.start { update in
                continuation.yield(update)
            }
        }

        consumerTask = Task { [weak self] in
            for await update in stream {
                guard let self else { return }
                self.handle(update: update)
            }
        }
    }

    private func handle(update: ClientUpdate) {
        switch update {
        case .connecting:
            logger.info("connecting")
            controller?.applyState(.connecting)
        case .connected(let snap):
            logger.debug("snapshot: \(snap.mounts.count, privacy: .public) mount(s)")
            controller?.applyState(.connected(snap))
        case .disconnected(let err):
            logger.warning("disconnected: \(err, privacy: .public)")
            controller?.applyState(.disconnected(err))
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        consumerTask?.cancel()
        let client = self.client
        Task { await client?.stop() }
    }
}
