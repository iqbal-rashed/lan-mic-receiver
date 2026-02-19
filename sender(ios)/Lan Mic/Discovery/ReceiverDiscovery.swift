//
//  ReceiverDiscovery.swift
//  Lan Mic
//
//  Uses Network.framework NWBrowser to discover LAN Mic Receiver
//  instances advertising via Bonjour (_lanmic._tcp).
//

import Network
import Foundation
import Combine
import os.log

// MARK: - Discovered Receiver

struct DiscoveredReceiver: Identifiable, Hashable {
    let id: String          // NWEndpoint description as stable ID
    let name: String        // Bonjour service name
    var host: String = ""   // Resolved IP address
    var port: UInt16 = 0    // Resolved port
    var isResolving: Bool = true

    var displayName: String {
        if host.isEmpty { return name }
        return "\(name) (\(host):\(port))"
    }
}

// MARK: - ReceiverDiscovery

final class ReceiverDiscovery: ObservableObject {
    @Published private(set) var receivers: [DiscoveredReceiver] = []
    @Published private(set) var isSearching = false

    private var browser: NWBrowser?
    private var resolveConnections: [String: NWConnection] = [:]
    private let logger = Logger(subsystem: "com.lanmic.app", category: "Discovery")
    private let queue = DispatchQueue(label: "com.lanmic.discovery", qos: .userInitiated)

    private static let serviceType = "_lanmic._tcp"

    // MARK: - Start / Stop

    func startBrowsing() {
        stopBrowsing()

        let params = NWParameters()
        params.includePeerToPeer = true

        let descriptor = NWBrowser.Descriptor.bonjour(type: Self.serviceType, domain: nil)
        let browser = NWBrowser(for: descriptor, using: params)

        browser.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            DispatchQueue.main.async {
                switch state {
                case .ready:
                    self.isSearching = true
                    self.logger.info("Browser ready — scanning for receivers")
                case .failed(let error):
                    self.logger.error("Browser failed: \(error.localizedDescription)")
                    self.isSearching = false
                case .cancelled:
                    self.isSearching = false
                default:
                    break
                }
            }
        }

        browser.browseResultsChangedHandler = { [weak self] results, changes in
            guard let self else { return }
            self.handleBrowseResults(results, changes: changes)
        }

        browser.start(queue: queue)
        self.browser = browser
        logger.info("Started browsing for \(Self.serviceType)")
    }

    func stopBrowsing() {
        browser?.cancel()
        browser = nil
        resolveConnections.values.forEach { $0.cancel() }
        resolveConnections.removeAll()
        DispatchQueue.main.async {
            self.receivers.removeAll()
            self.isSearching = false
        }
        logger.info("Stopped browsing")
    }

    // MARK: - Handle Results

    private func handleBrowseResults(_ results: Set<NWBrowser.Result>, changes: Set<NWBrowser.Result.Change>) {
        for change in changes {
            switch change {
            case .added(let result):
                handleAdded(result)
            case .removed(let result):
                handleRemoved(result)
            case .changed(old: _, new: let result, flags: _):
                handleRemoved(result)
                handleAdded(result)
            case .identical:
                break
            @unknown default:
                break
            }
        }
    }

    private func handleAdded(_ result: NWBrowser.Result) {
        let endpointID = "\(result.endpoint)"

        // Extract the name from the endpoint
        let name: String
        switch result.endpoint {
        case .service(let serviceName, _, _, _):
            name = serviceName
        default:
            name = endpointID
        }

        let receiver = DiscoveredReceiver(id: endpointID, name: name)
        DispatchQueue.main.async {
            self.receivers.removeAll { $0.id == endpointID }
            self.receivers.append(receiver)
        }
        logger.info("Discovered receiver: \(name)")

        // Resolve the endpoint to get host + port
        resolveEndpoint(result.endpoint, id: endpointID)
    }

    private func handleRemoved(_ result: NWBrowser.Result) {
        let endpointID = "\(result.endpoint)"
        resolveConnections[endpointID]?.cancel()
        resolveConnections.removeValue(forKey: endpointID)
        DispatchQueue.main.async {
            self.receivers.removeAll { $0.id == endpointID }
        }
        logger.info("Receiver removed: \(endpointID)")
    }

    // MARK: - Resolve Endpoint

    private func resolveEndpoint(_ endpoint: NWEndpoint, id: String) {
        // Create a TCP connection to resolve the Bonjour endpoint to an IP:port
        let connection = NWConnection(to: endpoint, using: .tcp)

        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                // Connection succeeded — extract the resolved endpoint
                if let innerEndpoint = connection.currentPath?.remoteEndpoint {
                    self.extractAddress(from: innerEndpoint, id: id)
                }
                connection.cancel()
                self.resolveConnections.removeValue(forKey: id)

            case .failed(let error):
                self.logger.warning("Resolve failed for \(id): \(error.localizedDescription)")
                connection.cancel()
                self.resolveConnections.removeValue(forKey: id)
                // Mark as not resolving even though it failed
                DispatchQueue.main.async {
                    if let idx = self.receivers.firstIndex(where: { $0.id == id }) {
                        self.receivers[idx].isResolving = false
                    }
                }

            case .cancelled:
                self.resolveConnections.removeValue(forKey: id)

            default:
                break
            }
        }

        resolveConnections[id] = connection
        connection.start(queue: queue)
    }

    private func extractAddress(from endpoint: NWEndpoint, id: String) {
        switch endpoint {
        case .hostPort(let host, let port):
            let hostStr: String
            switch host {
            case .ipv4(let addr):
                hostStr = "\(addr)"
            case .ipv6(let addr):
                hostStr = "\(addr)"
            case .name(let name, _):
                hostStr = name
            @unknown default:
                hostStr = "\(host)"
            }
            let portNum = port.rawValue

            DispatchQueue.main.async {
                if let idx = self.receivers.firstIndex(where: { $0.id == id }) {
                    self.receivers[idx].host = hostStr
                    self.receivers[idx].port = portNum
                    self.receivers[idx].isResolving = false
                    self.logger.info("Resolved \(self.receivers[idx].name) → \(hostStr):\(portNum)")
                }
            }

        default:
            logger.warning("Unexpected resolved endpoint type: \(String(describing: endpoint))")
        }
    }

    deinit {
        stopBrowsing()
    }
}
