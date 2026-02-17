//
//  ContentView.swift
//  Lan Mic
//
//  Main SwiftUI view — connection controls, live status, and help.
//

import SwiftUI

struct ContentView: View {
    @StateObject private var vm = ConnectionViewModel()

    var body: some View {
        NavigationView {
            ScrollView {
                VStack(spacing: 20) {
                    connectionCard
                    controlsCard
                    statusCard
                    helpCard
                }
                .padding()
            }
            .background(Color(.systemGroupedBackground))
            .onTapGesture {
                UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
            }
            .navigationTitle("LAN Mic")
        }
        .navigationViewStyle(.stack)
    }

    // MARK: - Connection Card

    private var connectionCard: some View {
        VStack(spacing: 14) {
            Label("Connection", systemImage: "wifi")
                .font(.headline)
                .frame(maxWidth: .infinity, alignment: .leading)

            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("PC IP Address")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    TextField("192.168.1.100", text: $vm.ip)
                        .keyboardType(.numbersAndPunctuation)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled(true)
                        .textFieldStyle(.roundedBorder)
                }

                VStack(alignment: .leading, spacing: 4) {
                    Text("Port")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    TextField("9001", text: $vm.port)
                        .keyboardType(.numberPad)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 80)
                }
            }

            Toggle(isOn: $vm.autoReconnect) {
                Label("Auto Reconnect", systemImage: "arrow.clockwise")
                    .font(.subheadline)
            }
            .tint(.blue)
        }
        .padding()
        .background(Color(.secondarySystemGroupedBackground))
        .cornerRadius(16)
    }

    // MARK: - Controls Card

    private var controlsCard: some View {
        HStack(spacing: 16) {
            Button {
                vm.start()
            } label: {
                Label("Connect", systemImage: "mic.fill")
                    .font(.headline)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
            }
            .buttonStyle(.borderedProminent)
            .tint(.blue)
            .disabled(isConnectDisabled)

            Button(role: .destructive) {
                vm.stop()
            } label: {
                Label("Stop", systemImage: "stop.fill")
                    .font(.headline)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
            }
            .buttonStyle(.borderedProminent)
            .tint(.red)
            .disabled(vm.state == .idle)
        }
        .padding()
        .background(Color(.secondarySystemGroupedBackground))
        .cornerRadius(16)
    }

    private var isConnectDisabled: Bool {
        vm.ip.isEmpty || (vm.state != .idle && vm.state != .failed)
    }

    // MARK: - Status Card

    private var statusCard: some View {
        VStack(spacing: 12) {
            Label("Status", systemImage: "antenna.radiowaves.left.and.right")
                .font(.headline)
                .frame(maxWidth: .infinity, alignment: .leading)

            // Connection state
            HStack {
                Circle()
                    .fill(colorForState(vm.state))
                    .frame(width: 12, height: 12)
                    .shadow(color: colorForState(vm.state).opacity(0.6), radius: 4)
                Text(vm.state.rawValue)
                    .fontWeight(.medium)
                Spacer()
            }

            Divider()

            statusRow(label: "ICE State", value: vm.iceState)
            statusRow(label: "Packets Sent", value: "\(vm.packetsSent)")

            if !vm.lastError.isEmpty {
                HStack(alignment: .top) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(.red)
                        .font(.caption)
                    Text(vm.lastError)
                        .font(.caption)
                        .foregroundColor(.red)
                    Spacer()
                }
                .padding(.top, 4)
            }
        }
        .padding()
        .background(Color(.secondarySystemGroupedBackground))
        .cornerRadius(16)
    }

    private func statusRow(label: String, value: String) -> some View {
        HStack {
            Text(label)
                .foregroundColor(.secondary)
                .font(.subheadline)
            Spacer()
            Text(value)
                .font(.subheadline)
                .fontWeight(.medium)
                .monospacedDigit()
        }
    }

    // MARK: - Help Card

    private var helpCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Help", systemImage: "questionmark.circle")
                .font(.headline)

            Text("1. Start the Windows Rust receiver on your PC")
            Text("2. Make sure it's listening on the specified port")
            Text("3. Enter your PC's LAN IP and press Connect")

            HStack(spacing: 4) {
                Image(systemName: "link")
                    .font(.caption)
                    .foregroundColor(.secondary)
                Text("ws://\(vm.ip.isEmpty ? "IP" : vm.ip):\(vm.port)/ws")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .monospaced()
            }
            .padding(.top, 4)
        }
        .font(.subheadline)
        .foregroundColor(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(Color(.secondarySystemGroupedBackground))
        .cornerRadius(16)
    }

    // MARK: - Helpers

    private func colorForState(_ state: ConnectionViewModel.State) -> Color {
        switch state {
        case .idle: return .gray
        case .connecting, .exchangingSDP, .iceConnecting: return .orange
        case .connected: return .green
        case .failed: return .red
        }
    }
}

#Preview {
    ContentView()
}
