//
//  WebRTCManager.swift
//  Lan Mic
//
//  Manages RTCPeerConnection for audio-only WebRTC streaming.
//  Creates local audio track from microphone and handles ICE/SDP negotiation.
//

import Foundation
import WebRTC

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

    private static let factory: RTCPeerConnectionFactory = {
        RTCInitializeSSL()
        let encoderFactory = RTCDefaultVideoEncoderFactory()
        let decoderFactory = RTCDefaultVideoDecoderFactory()
        return RTCPeerConnectionFactory(
            encoderFactory: encoderFactory,
            decoderFactory: decoderFactory
        )
    }()

    private var peerConnection: RTCPeerConnection?
    private var localAudioTrack: RTCAudioTrack?

    // Toggle: set to true to add a STUN server for non-LAN use
    private let useSTUN = false

    // MARK: - Setup

    func createPeerConnection() {
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

        let pc = WebRTCManager.factory.peerConnection(
            with: config,
            constraints: constraints,
            delegate: self
        )

        peerConnection = pc
        addLocalAudioTrack()
        print("[WebRTC] Peer connection created")
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
        print("[WebRTC] Local audio track added")
    }

    // MARK: - SDP Negotiation

    func setRemoteOffer(_ sdp: String, completion: @escaping (Error?) -> Void) {
        let sessionDescription = RTCSessionDescription(type: .offer, sdp: sdp)
        peerConnection?.setRemoteDescription(sessionDescription) { error in
            if let error {
                print("[WebRTC] setRemoteDescription error: \(error.localizedDescription)")
            } else {
                print("[WebRTC] Remote offer set")
            }
            completion(error)
        }
    }

    func createAnswer(completion: @escaping (String?, Error?) -> Void) {
        let constraints = RTCMediaConstraints(
            mandatoryConstraints: [
                kRTCMediaConstraintsOfferToReceiveAudio: kRTCMediaConstraintsValueTrue,
                kRTCMediaConstraintsOfferToReceiveVideo: kRTCMediaConstraintsValueFalse
            ],
            optionalConstraints: nil
        )

        peerConnection?.answer(for: constraints) { [weak self] answer, error in
            guard let self, let answer else {
                completion(nil, error)
                return
            }

            self.peerConnection?.setLocalDescription(answer) { error in
                if let error {
                    print("[WebRTC] setLocalDescription error: \(error.localizedDescription)")
                    completion(nil, error)
                } else {
                    print("[WebRTC] Local answer set")
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
        guard let pc = peerConnection else { return }
        pc.add(iceCandidate)
    }

    // MARK: - Stats

    func getStats(completion: @escaping (_ packetsSent: Int) -> Void) {
        peerConnection?.statistics { report in
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
        localAudioTrack?.isEnabled = false
        localAudioTrack = nil
        peerConnection?.close()
        peerConnection = nil
        print("[WebRTC] Peer connection closed")
    }
}

// MARK: - RTCPeerConnectionDelegate

extension WebRTCManager: RTCPeerConnectionDelegate {
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCSignalingState) {
        print("[WebRTC] Signaling state: \(stateChanged.rawValue)")
        delegate?.webRTCManager(self, didChangeSignalingState: stateChanged)
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {
        print("[WebRTC] Remote stream added")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove stream: RTCMediaStream) {
        print("[WebRTC] Remote stream removed")
    }

    func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {
        print("[WebRTC] Negotiation needed")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceConnectionState) {
        print("[WebRTC] ICE connection state: \(newState.rawValue)")
        delegate?.webRTCManager(self, didChangeICEState: newState)
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceGatheringState) {
        print("[WebRTC] ICE gathering state: \(newState.rawValue)")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didGenerate candidate: RTCIceCandidate) {
        print("[WebRTC] ICE candidate generated: \(candidate.sdp)")
        delegate?.webRTCManager(self, didGenerateICECandidate: candidate)
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove candidates: [RTCIceCandidate]) {
        print("[WebRTC] ICE candidates removed")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didOpen dataChannel: RTCDataChannel) {
        print("[WebRTC] Data channel opened")
    }

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCPeerConnectionState) {
        print("[WebRTC] Connection state: \(stateChanged.rawValue)")
        delegate?.webRTCManager(self, didChangeConnectionState: stateChanged)
    }
}
