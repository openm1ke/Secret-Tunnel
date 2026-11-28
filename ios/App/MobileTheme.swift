import SwiftUI

enum Appearance: String, CaseIterable {
    case system, light, dark

    var label: String {
        switch self {
        case .system: return tr("Системная")
        case .light: return tr("Светлая")
        case .dark: return tr("Тёмная")
        }
    }

    var colorScheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }
}

struct MobilePalette {
    let background: Color
    let surface: Color
    let raised: Color
    let border: Color
    let text: Color
    let muted: Color
    let accent: Color
    let onAccent: Color
    let upload: Color
    let warning: Color
    let error: Color

    // Match Android's MobileUi.kt so the connection states read the same on both platforms.
    static let light = MobilePalette(
        background: color(0xedf3f2), surface: color(0xffffff), raised: color(0xdfedeb),
        border: color(0xc5d7d5), text: color(0x142e32), muted: color(0x4c6569),
        accent: color(0x08706c), onAccent: color(0xffffff), upload: color(0x543bb7),
        warning: color(0x8a5300), error: color(0xa42324)
    )

    static let dark = MobilePalette(
        background: color(0x081417), surface: color(0x102327), raised: color(0x193135),
        border: color(0x2a4449), text: color(0xeaf6f2), muted: color(0x9db5b6),
        accent: color(0x79e4d4), onAccent: color(0x081417), upload: color(0xadb7ff),
        warning: color(0xffcd7e), error: color(0xff9d9d)
    )

    private static func color(_ hex: UInt32) -> Color {
        Color(
            red: Double((hex >> 16) & 255) / 255,
            green: Double((hex >> 8) & 255) / 255,
            blue: Double(hex & 255) / 255
        )
    }
}

struct MobileButtonStyle: ButtonStyle {
    let palette: MobilePalette
    var primary = false
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.subheadline.weight(.semibold))
            .multilineTextAlignment(.center)
            .padding(.horizontal, 12)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, minHeight: 48)
            .foregroundColor(primary ? palette.onAccent : palette.accent)
            .background(
                RoundedRectangle(cornerRadius: 14).fill(primary ? palette.accent : palette.raised)
            )
            .opacity(isEnabled ? (configuration.isPressed ? 0.77 : 1) : 0.48)
    }
}
