//
//  AudioSessionManager.swift
//  Lan Mic
//
//  Manages AVAudioSession configuration, interruption handling,
//  and audio route changes for WebRTC microphone streaming.
//

import AVFoundation
import Foundation

final class AudioSessionManager {
    static let shared = AudioSessionManager()

    private let session = AVAudioSession.sharedInstance()
    private var isActive = false

    private init() {
        setupNotifications()
    }

    // MARK: - Activate / Deactivate

    func activate() throws {
        try session.setCategory(
            .playAndRecord,
            mode: .voiceChat,
            options: [.allowBluetooth, .defaultToSpeaker]
        )
        try session.setActive(true, options: [])
        isActive = true
        print("[AudioSession] Activated — category: \(session.category.rawValue), mode: \(session.mode.rawValue)")
    }

    func deactivate() {
        guard isActive else { return }
        do {
            try session.setActive(false, options: .notifyOthersOnDeactivation)
            isActive = false
            print("[AudioSession] Deactivated")
        } catch {
            print("[AudioSession] Deactivation error: \(error.localizedDescription)")
        }
    }

    // MARK: - Notifications

    private func setupNotifications() {
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleInterruption(_:)),
            name: AVAudioSession.interruptionNotification,
            object: session
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleRouteChange(_:)),
            name: AVAudioSession.routeChangeNotification,
            object: session
        )
    }

    @objc private func handleInterruption(_ notification: Notification) {
        guard let userInfo = notification.userInfo,
              let typeValue = userInfo[AVAudioSessionInterruptionTypeKey] as? UInt,
              let type = AVAudioSession.InterruptionType(rawValue: typeValue)
        else { return }

        switch type {
        case .began:
            print("[AudioSession] Interruption began (phone call, Siri, etc.)")
        case .ended:
            print("[AudioSession] Interruption ended — attempting recovery")
            let options = userInfo[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            if AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume) {
                try? session.setActive(true, options: [])
                print("[AudioSession] Resumed after interruption")
            }
        @unknown default:
            break
        }
    }

    @objc private func handleRouteChange(_ notification: Notification) {
        guard let userInfo = notification.userInfo,
              let reasonValue = userInfo[AVAudioSessionRouteChangeReasonKey] as? UInt,
              let reason = AVAudioSession.RouteChangeReason(rawValue: reasonValue)
        else { return }

        print("[AudioSession] Route changed: \(reason.rawValue)")

        switch reason {
        case .oldDeviceUnavailable:
            // Headphones unplugged, etc. — try to recover
            try? session.setActive(true, options: [])
        default:
            break
        }
    }
}
