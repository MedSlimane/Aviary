//
//  AviaryApp.swift
//  Aviary
//

import SwiftUI

@main
struct AviaryApp: App {
    #if DEBUG
    @State private var model = AppModel.fromLaunchArguments()
    #else
    @State private var model = AppModel()
    #endif

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(.dark)
                .tint(Ink.violet)
        }
    }
}
