//
//  MCPServersView.swift
//  Aviary
//

import SwiftUI

struct MCPServersView: View {
    let servers: [MCPServer]

    /// Local copy so the toggles are live without a backend.
    @State private var working: [MCPServer] = []

    private var attentionCount: Int {
        working.filter {
            if case .ok = $0.health { return false }
            return true
        }.count
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(subtitle)
                    .font(.subheadline)
                    .foregroundStyle(Ink.secondary)

                VStack(spacing: 0) {
                    ForEach($working) { $server in
                        ServerRow(server: $server)
                        if server.id != working.last?.id {
                            Divider().overlay(Ink.borderSubtle)
                                .padding(.leading, 36)
                        }
                    }
                }
                .background(Ink.elevated.opacity(0.7), in: .rect(cornerRadius: Radius.lg))
                .overlay(
                    RoundedRectangle(cornerRadius: Radius.lg)
                        .stroke(Ink.borderSubtle, lineWidth: 1)
                )

                Text("Toggles apply to Claude Code. Switch runner from the header.")
                    .font(.caption)
                    .foregroundStyle(Ink.tertiary)
                    .padding(.horizontal, 4)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 32)
        }
        .navigationTitle("MCP Servers")
        .navigationBarTitleDisplayMode(.large)
        .onAppear {
            // Seed once; re-entering the screen keeps the user's toggles.
            if working.isEmpty { working = servers }
        }
    }

    private var subtitle: String {
        let count = working.count
        guard attentionCount > 0 else { return "\(count) servers · all healthy" }
        return "\(count) servers · \(attentionCount) need\(attentionCount == 1 ? "s" : "") attention"
    }
}

private struct ServerRow: View {
    @Binding var server: MCPServer

    var body: some View {
        HStack(spacing: 12) {
            Circle()
                .fill(server.health.tint)
                .frame(width: 10, height: 10)

            VStack(alignment: .leading, spacing: 2) {
                Text(server.name)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(Ink.primary)
                Text(server.health.detail)
                    .font(.caption)
                    .foregroundStyle(Ink.secondary)
            }

            Spacer(minLength: 0)

            Toggle("Enable \(server.name)", isOn: $server.isEnabled)
                .labelsHidden()
                .tint(Ink.ok)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .sensoryFeedback(.selection, trigger: server.isEnabled)
    }
}

#Preview {
    NavigationStack {
        MCPServersView(servers: MCPServer.sample)
            .background { AuroraBackground(mood: .tidal) }
    }
}
