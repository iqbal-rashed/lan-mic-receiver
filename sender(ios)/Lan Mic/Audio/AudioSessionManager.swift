//
//  AudioSessionManager.swift
//  Lan Mic
//
//  Manages AVAudioSession configuration, interruption handling,
//  and audio route changes for WebRTC microphone streaming.
//  Configured for reliable background audio operation.
//

import AVFoundation
import UIKit
import os.log

final class AudioSessionManager {
    static let shared = AudioSessionManager()

    private let session = AVAudioSession.sharedInstance()
    private(set) var isActive = false
    private var backgroundTaskID: UIBackgroundTaskIdentifier = .invalid
    private let logger = Logger(subsystem: "com.lanmic.app", category: "AudioSession")

    /// Posted when an interruption begins or the session is forcibly deactivated.
    static let didInterruptNotification = Notification.Name("AudioSessionDidInterrupt")
    /// Posted when an interruption ends and audio has been resumed.
    static let didResumeNotification = Notification.Name("AudioSessionDidResume")

    private init() {
        setupNotifications()
    }

    // MARK: - Activate / Deactivate

    func activate() throws {
        // .measurement avoids iOS voice-chat ducking/processing.
        // .mixWithOthers helps the session survive backgrounding.
        try session.setCategory(
            .playAndRecord,
            mode: .measurement,
            options: [.allowBluetoothHFP, .defaultToSpeaker, .mixWithOthers]
        )
        try session.setPreferredSampleRate(48_000)
        try session.setPreferredIOBufferDuration(0.02)  // 20 ms
        try session.setActive(true, options: [])
        isActive = true

        // Begin a background task so iOS doesn't suspend us immediately.
        beginBackgroundTask()

        logger.info("Activated — category: \(self.session.category.rawValue), mode: \(self.session.mode.rawValue), sampleRate: \(self.session.sampleRate)")
    }

    func deactivate() {
        guard isActive else { return }
        do {
            try session.setActive(false, options: .notifyOthersOnDeactivation)
            isActive = false
            endBackgroundTask()
            logger.info("Deactivated")
        } catch {
            logger.error("Deactivation error: \(error.localizedDescription)")
        }
    }

    // MARK: - Background Task

    private func beginBackgroundTask() {
        guard backgroundTaskID == .invalid else { return }
        backgroundTaskID = UIApplication.shared.beginBackgroundTask(withName: "LanMicAudio") { [weak self] in
            self?.logger.warning("Background task expired by system")
            self?.endBackgroundTask()
        }
        logger.debug("Background task started: \(self.backgroundTaskID.rawValue)")
    }

    private func endBackgroundTask() {
        guard backgroundTaskID != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTaskID)
        backgroundTaskID = .invalid
        logger.debug("Background task ended")
    }

    // MARK: - Notifications

    private func setupNotifications() {
        let nc = NotificationCenter.default
        nc.addObserver(
            self,
            selector: #selector(handleInterruption(_:)),
            name: AVAudioSession.interruptionNotification,
            object: session
        )
        nc.addObserver(
            self,
            selector: #selector(handleRouteChange(_:)),
            name: AVAudioSession.routeChangeNotification,
            object: session
        )
        nc.addObserver(
            self,
            selector: #selector(handleMediaServicesReset(_:)),
            name: AVAudioSession.mediaServicesWereResetNotification,
            object: nil
        )
    }

    @objc private func handleInterruption(_ notification: Notification) {
        guard let userInfo = notification.userInfo,
              let typeValue = userInfo[AVAudioSessionInterruptionTypeKey] as? UInt,
              let type = AVAudioSession.InterruptionType(rawValue: typeValue)
        else { return }

        switch type {
        case .began:
            logger.info("Interruption began (phone call, Siri, etc.)")
            NotificationCenter.default.post(name: Self.didInterruptNotification, object: self)

        case .ended:
            logger.info("Interruption ended — attempting recovery")
            let options = userInfo[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            if AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume) {
                do {
                    try session.setActive(true, options: [])
                    isActive = true
                    beginBackgroundTask()
                    logger.info("Resumed after interruption")
                    NotificationCenter.default.post(name: Self.didResumeNotification, object: self)
                } catch {
                    logger.error("Failed to resume after interruption: \(error.localizedDescription)")
                }
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

        let currentRoute = session.currentRoute
        let inputs = currentRoute.inputs.map(\.portName).joined(separator: ", ")
        logger.info("Route changed: reason=\(reason.rawValue), inputs=[\(inputs)]")

        switch reason {
        case .oldDeviceUnavailable:
            // Headphones unplugged, etc. — try to recover
            logger.info("Old device unavailable — attempting recovery")
            try? session.setActive(true, options: [])
        default:
            break
        }
    }

    @objc private func handleMediaServicesReset(_ notification: Notification) {
        // Media services were reset (rare). We need to reconfigure everything.
        logger.warning("Media services reset — reconfiguring audio session")
        isActive = false
        endBackgroundTask()
        try? activate()
    }
}
