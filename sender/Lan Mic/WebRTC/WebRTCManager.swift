//
//  WebRTCManager.swift
//  Lan Mic
//
//  Manages RTCPeerConnection for audio-only WebRTC streaming.
//  Creates local audio track from microphone and handles ICE/SDP negotiation.
//  Optimised for audio-only: no video encoder/decoder factories.
//

import Foundation
import WebRTC
import os.log

// MARK: - Delegate

protocol WebRTCManagerDelegate: AnyObject {
    func webRTCManager(_ manager: WebRTCManager, didGenerateICECandidate candidate: RTCIceCandidate)
    func webRTCManager(_ manager: WebRTCManager, didChangeConnectionState state: RTCPeerConnectionState)
    func webRTCManager(_ manager: WebRTCManager, didChangeICEState state: RTCIceConnectionState)
    func webRTCManager(_ manager: WebRTCManager, didChangeSignalingState state: RTCSignalingState)
}

// MARK: - WebRTCManager

final class WebRTCManager: NSObject {
    weak var delegate: WebRTCManagerDelegate?

    private let logger = Logger(subsystem: "com.lanmic.app", category: "WebRTC")

    // Audio-only factory — no video codecs needed, saves memory
    private static let factory: RTCPeerConnectionFactory = {
        RTCInitializeSSL()
        return RTCPeerConnectionFactory(
            encoderFactory: RTCAudioEncoderFactory(),
            decoderFactory: RTCAudioDecoderFactory()
        )
    }()

    private var peerConnection: RTCPeerConnection?
    private var localAudioTrack: RTCAudioTrack?

    // Toggle: set to true to add a STUN server for non-LAN use
    private let useSTUN = false

    // MARK: - Setup

    func createPeerConnection() {
        // Clean up any existing connection first
        close()

        let config = RTCConfiguration()

        // LAN-only: empty ICE servers
        if useSTUN {
            config.iceServers = [RTCIceServer(urlStrings: ["stun:stun.l.google.com:19302"])]
        } else {
            config.iceServers = []
        }

        config.sdpSemantics = .unifiedPlan
        config.continualGatheringPolicy = .gatherContinually
        config.candidateNetworkPolicy = .all

        let constraints = RTCMediaConstraints(
            mandatoryConstraints: nil,
            optionalConstraints: nil
        )

        guard let pc = WebRTCManager.factory.peerConnection(
            with: config,
            constraints: constraints,
            delegate: self
        ) else {
            logger.error("Failed to create peer connection")
            return
        }

        peerConnection = pc
        addLocalAudioTrack()
        logger.info("Peer connection created")
    }

    private func addLocalAudioTrack() {
        let audioConstraints = RTCMediaConstraints(
            mandatoryConstraints: nil,
            optionalConstraints: [
                "googEchoCancellation": "true",
                "googNoiseSuppression": "true",
                "googAutoGainControl": "true",
                "googHighpassFilter": "true"
            ]
        )

        let audioSource = WebRTCManager.factory.audioSource(with: audioConstraints)
        let audioTrack = WebRTCManager.factory.audioTrack(with: audioSource, trackId: "mic0")
        audioTrack.isEnabled = true

        peerConnection?.add(audioTrack, streamIds: ["stream0"])
        localAudioTrack = audioTrack
        logger.info("Local audio track added")
    }

    // MARK: - SDP Negotiation

    func setRemoteOffer(_ sdp: String, completion: @escaping (Error?) -> Void) {
        guard let pc = peerConnection else {
            completion(NSError(domain: "WebRTCManager", code: -1, userInfo: [NSLocalizedDescriptionKey: "No peer connection"]))
            return
        }

        let sessionDescription = RTCSessionDescription(type: .offer, sdp: sdp)
        pc.setRemoteDescription(sessionDescription) { [weak self] error in
            if let error {
                self?.logger.error("setRemoteDescription error: \(error.localizedDescription)")
            } else {
                self?.logger.info("Remote offer set")
            }
            completion(error)
        }
    }

    func createAnswer(completion: @escaping (String?, Error?) -> Void) {
        guard let pc = peerConnection else {
            completion(nil, NSError(domain: "WebRTCManager", code: -1, userInfo: [NSLocalizedDescriptionKey: "No peer connection"]))
            return
        }

        let constraints = RTCMediaConstraints(
            mandatoryConstraints: [
                kRTCMediaConstraintsOfferToReceiveAudio: kRTCMediaConstraintsValueTrue,
                kRTCMediaConstraintsOfferToReceiveVideo: kRTCMediaConstraintsValueFalse
            ],
            optionalConstraints: nil
        )

        pc.answer(for: constraints) { [weak self] answer, error in
            guard let self, let answer else {
                completion(nil, error)
                return
            }

            self.peerConnection?.setLocalDescription(answer) { [weak self] error in
                if let error {
                    self?.logger.error("setLocalDescription error: \(error.localizedDescription)")
                    completion(nil, error)
                } else {
                    self?.logger.info("Local answer set")
                    completion(answer.sdp, nil)
                }
            }
        }
    }

    // MARK: - ICE

    func addICECandidate(candidate: String, sdpMid: String?, sdpMLineIndex: Int32?) {
        let iceCandidate = RTCIceCandidate(
            sdp: candidate,
            sdpMLineIndex: sdpMLineIndex ?? 0,
            sdpMid: sdpMid
        )
        guard let pc = peerConnection else {
            logger.warning("addICECandidate called but no peer connection exists")
            return
        }
        pc.add(iceCandidate)
    }

    // MARK: - Stats

    func getStats(completion: @escaping (_ packetsSent: Int) -> Void) {
        guard let pc = peerConnection else {
            completion(0)
            return
        }
        pc.statistics { report in
            var totalPackets = 0
            for (_, stats) in report.statistics {
                if stats.type == "outbound-rtp" {
                    if let packets = stats.values["packetsSent"] as? Int {
                        totalPackets += packets
                    } else if let packetsStr = stats.values["packetsSent"] as? String,
                              let packets = Int(packetsStr) {
                        totalPackets += packets
                    }
                }
            }
            completion(totalPackets)
        }
    }

    // MARK: - Teardown

    func close() {
        if let track = localAudioTrack {
            track.isEnabled = false
            // Remove the track from the peer connection senders
            if let pc = peerConnection {
                for sender in pc.senders {
                    if sender.track?.trackId == track.trackId {
                        pc.removeTrack(sender)
                    }
                }
            }
        }
        localAudioTrack = nil
        peerConnection?.close()
        peerConnection = nil
        logger.info("Peer connection closed")
    }

    deinit {
        // Safety: ensure cleanup even if close() was not called
        localAudioTrack?.isEnabled = false
        localAudioTrack = nil
        peerConnection?.close()
        peerConnection = nil
    }
}

// MARK: - RTCPeerConnectionDelegate

extension WebRTCManager: RTCPeerConnectionDelegate {
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCSignalingState) {
        logger.debug("Signaling state: \(String(describing: stateChanged))")
        delegate?.webRTCManager(self, didChangeSignalingState: stateChanged)
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {
        logger.info("Remote stream added")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove stream: RTCMediaStream) {
        logger.info("Remote stream removed")
    }

    func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {
        logger.debug("Negotiation needed")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceConnectionState) {
        logger.debug("ICE connection state: \(String(describing: newState))")
        delegate?.webRTCManager(self, didChangeICEState: newState)
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceGatheringState) {
        logger.debug("ICE gathering state: \(String(describing: newState))")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didGenerate candidate: RTCIceCandidate) {
        logger.debug("ICE candidate generated")
        delegate?.webRTCManager(self, didGenerateICECandidate: candidate)
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove candidates: [RTCIceCandidate]) {
        logger.debug("ICE candidates removed")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didOpen dataChannel: RTCDataChannel) {
        logger.info("Data channel opened")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCPeerConnectionState) {
        logger.info("Connection state: \(String(describing: stateChanged))")
        delegate?.webRTCManager(self, didChangeConnectionState: stateChanged)
    }
}

// MARK: - Audio-only factory stubs

/// Minimal encoder factory for audio-only (avoids loading video codecs)
private class RTCAudioEncoderFactory: NSObject, RTCVideoEncoderFactory {
    func createEncoder(_ info: RTCVideoCodecInfo) -> (any RTCVideoEncoder)? { nil }
    func supportedCodecs() -> [RTCVideoCodecInfo] { [] }
}

/// Minimal decoder factory for audio-only (avoids loading video codecs)
private class RTCAudioDecoderFactory: NSObject, RTCVideoDecoderFactory {
    func createDecoder(_ info: RTCVideoCodecInfo) -> (any RTCVideoDecoder)? { nil }
    func supportedCodecs() -> [RTCVideoCodecInfo] { [] }
}
