//
//  Lan_MicApp.swift
//  Lan Mic
//
//  App entry point. Configures audio session on launch.
//

import SwiftUI
import AVFoundation
import os.log

@main
struct Lan_MicApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

// MARK: - App Delegate

class AppDelegate: NSObject, UIApplicationDelegate {
    private let logger = Logger(subsystem: "com.lanmic.app", category: "AppDelegate")

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        // Request microphone permission early
        AVAudioSession.sharedInstance().requestRecordPermission { [weak self] granted in
            if granted {
                self?.logger.info("Microphone permission granted")
            } else {
                self?.logger.warning("Microphone permission denied")
            }
        }
        return true
    }
}
