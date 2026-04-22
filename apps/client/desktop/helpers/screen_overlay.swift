#!/usr/bin/env swift
// screen_overlay.swift — Brief visual indicator for computer_use actions.
// Usage: screen_overlay <action> [args...]
//   click <x> <y>
//   double_click <x> <y>
//   right_click <x> <y>
//   type <x> <y> <text>
//   keypress <keys>
//   scroll <x> <y> <dx> <dy>
//   drag <x1> <y1> <x2> <y2>
//   move <x> <y>
//   screenshot

import Cocoa

// MARK: - Overlay Window

class OverlayWindow: NSWindow {
    init(frame: NSRect) {
        super.init(
            contentRect: frame,
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )
        self.level = .screenSaver
        self.isOpaque = false
        self.backgroundColor = .clear
        self.ignoresMouseEvents = true
        self.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        self.hasShadow = false
    }
}

// MARK: - Overlay Views

class ClickOverlayView: NSView {
    let color: NSColor
    let radius: CGFloat

    init(frame: NSRect, color: NSColor, radius: CGFloat = 20) {
        self.color = color
        self.radius = radius
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        // Outer ring
        let ringRect = bounds.insetBy(dx: 2, dy: 2)
        let path = NSBezierPath(ovalIn: ringRect)
        color.withAlphaComponent(0.3).setFill()
        path.fill()
        color.withAlphaComponent(0.8).setStroke()
        path.lineWidth = 2.5
        path.stroke()

        // Center dot
        let dotSize: CGFloat = 6
        let dotRect = NSRect(
            x: bounds.midX - dotSize / 2,
            y: bounds.midY - dotSize / 2,
            width: dotSize,
            height: dotSize
        )
        let dot = NSBezierPath(ovalIn: dotRect)
        color.setFill()
        dot.fill()
    }
}

class LabelOverlayView: NSView {
    let text: String
    let bgColor: NSColor

    init(frame: NSRect, text: String, bgColor: NSColor) {
        self.text = text
        self.bgColor = bgColor
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 6, yRadius: 6)
        bgColor.withAlphaComponent(0.85).setFill()
        path.fill()

        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 12, weight: .medium),
            .foregroundColor: NSColor.white,
        ]
        let str = NSAttributedString(string: text, attributes: attrs)
        let textSize = str.size()
        let textOrigin = NSPoint(
            x: bounds.midX - textSize.width / 2,
            y: bounds.midY - textSize.height / 2
        )
        str.draw(at: textOrigin)
    }
}

class DragOverlayView: NSView {
    let fromPoint: NSPoint
    let toPoint: NSPoint

    init(frame: NSRect, from: NSPoint, to: NSPoint) {
        self.fromPoint = from
        self.toPoint = to
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath()
        path.move(to: fromPoint)
        path.line(to: toPoint)
        NSColor.systemOrange.withAlphaComponent(0.8).setStroke()
        path.lineWidth = 2
        path.setLineDash([6, 4], count: 2, phase: 0)
        path.stroke()

        // Start dot
        let startDot = NSBezierPath(ovalIn: NSRect(x: fromPoint.x - 4, y: fromPoint.y - 4, width: 8, height: 8))
        NSColor.systemOrange.setFill()
        startDot.fill()

        // End arrow dot
        let endDot = NSBezierPath(ovalIn: NSRect(x: toPoint.x - 5, y: toPoint.y - 5, width: 10, height: 10))
        NSColor.systemRed.setFill()
        endDot.fill()
    }
}

class ScreenFlashView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.white.withAlphaComponent(0.15).setFill()
        bounds.fill()
    }
}

// MARK: - Helpers

/// Convert screen Y: our input uses top-left origin, macOS uses bottom-left
func flipY(_ y: CGFloat) -> CGFloat {
    guard let screen = NSScreen.main else { return y }
    return screen.frame.height - y
}

func showAndFade(_ window: OverlayWindow, duration: TimeInterval = 0.6) {
    window.alphaValue = 1.0
    window.orderFrontRegardless()

    NSAnimationContext.runAnimationGroup({ ctx in
        ctx.duration = duration
        window.animator().alphaValue = 0.0
    }, completionHandler: {
        NSApplication.shared.terminate(nil)
    })
}

// MARK: - Action Handlers

func showClick(x: CGFloat, y: CGFloat, color: NSColor, label: String? = nil) {
    let size: CGFloat = 44
    let flippedY = flipY(y)
    let frame = NSRect(x: x - size / 2, y: flippedY - size / 2, width: size, height: size)
    let window = OverlayWindow(frame: frame)
    let view = ClickOverlayView(frame: NSRect(origin: .zero, size: frame.size), color: color)
    window.contentView = view

    if let label = label {
        let labelWidth: CGFloat = CGFloat(label.count) * 8 + 20
        let labelFrame = NSRect(x: x - labelWidth / 2, y: flippedY - size / 2 - 24, width: labelWidth, height: 20)
        let labelWindow = OverlayWindow(frame: labelFrame)
        let labelView = LabelOverlayView(frame: NSRect(origin: .zero, size: labelFrame.size), text: label, bgColor: color)
        labelWindow.contentView = labelView
        labelWindow.alphaValue = 1.0
        labelWindow.orderFrontRegardless()

        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = 0.7
            labelWindow.animator().alphaValue = 0.0
        })
    }

    showAndFade(window, duration: 0.7)
}

func showLabel(text: String, color: NSColor, x: CGFloat? = nil, y: CGFloat? = nil) {
    let displayText = text.count > 40 ? String(text.prefix(37)) + "..." : text
    let labelWidth = max(CGFloat(displayText.count) * 8 + 24, 60)
    let labelHeight: CGFloat = 28

    let posX: CGFloat
    let posY: CGFloat
    if let x = x, let y = y {
        posX = x - labelWidth / 2
        posY = flipY(y) - labelHeight - 10
    } else {
        // Center of screen
        guard let screen = NSScreen.main else { return }
        posX = screen.frame.midX - labelWidth / 2
        posY = screen.frame.midY + 100
    }

    let frame = NSRect(x: posX, y: posY, width: labelWidth, height: labelHeight)
    let window = OverlayWindow(frame: frame)
    let view = LabelOverlayView(
        frame: NSRect(origin: .zero, size: frame.size),
        text: displayText,
        bgColor: color
    )
    window.contentView = view
    showAndFade(window, duration: 0.8)
}

func showDrag(x1: CGFloat, y1: CGFloat, x2: CGFloat, y2: CGFloat) {
    let minX = min(x1, x2) - 10
    let minY = min(flipY(y1), flipY(y2)) - 10
    let maxX = max(x1, x2) + 10
    let maxY = max(flipY(y1), flipY(y2)) + 10
    let frame = NSRect(x: minX, y: minY, width: maxX - minX, height: maxY - minY)

    let localFrom = NSPoint(x: x1 - minX, y: flipY(y1) - minY)
    let localTo = NSPoint(x: x2 - minX, y: flipY(y2) - minY)

    let window = OverlayWindow(frame: frame)
    let view = DragOverlayView(frame: NSRect(origin: .zero, size: frame.size), from: localFrom, to: localTo)
    window.contentView = view
    showAndFade(window, duration: 0.8)
}

func showScreenFlash() {
    guard let screen = NSScreen.main else { return }
    let window = OverlayWindow(frame: screen.frame)
    let view = ScreenFlashView(frame: NSRect(origin: .zero, size: screen.frame.size))
    window.contentView = view
    showAndFade(window, duration: 0.4)
}

// MARK: - Main

let app = NSApplication.shared
let args = CommandLine.arguments

guard args.count >= 2 else {
    fputs("Usage: screen_overlay <action> [args...]\n", stderr)
    exit(1)
}

let action = args[1]

// Dispatch to main run loop
DispatchQueue.main.async {
    switch action {
    case "click":
        guard args.count >= 4, let x = Double(args[2]), let y = Double(args[3]) else {
            NSApplication.shared.terminate(nil); return
        }
        showClick(x: CGFloat(x), y: CGFloat(y), color: .systemRed)

    case "double_click":
        guard args.count >= 4, let x = Double(args[2]), let y = Double(args[3]) else {
            NSApplication.shared.terminate(nil); return
        }
        showClick(x: CGFloat(x), y: CGFloat(y), color: .systemBlue, label: "double")

    case "right_click":
        guard args.count >= 4, let x = Double(args[2]), let y = Double(args[3]) else {
            NSApplication.shared.terminate(nil); return
        }
        showClick(x: CGFloat(x), y: CGFloat(y), color: .systemGreen, label: "right")

    case "type":
        let text = args.count >= 5 ? args[4] : ""
        let x = args.count >= 4 ? Double(args[2]) : nil
        let y = args.count >= 4 ? Double(args[3]) : nil
        let displayText = text.isEmpty ? "type" : text
        showLabel(
            text: displayText,
            color: .systemPurple,
            x: x.map { CGFloat($0) },
            y: y.map { CGFloat($0) }
        )

    case "keypress":
        let keys = args.count >= 3 ? args[2] : "keys"
        showLabel(text: keys, color: .systemIndigo)

    case "scroll":
        guard args.count >= 4, let x = Double(args[2]), let y = Double(args[3]) else {
            NSApplication.shared.terminate(nil); return
        }
        let dy = args.count >= 6 ? (args[5]) : "0"
        showClick(x: CGFloat(x), y: CGFloat(y), color: .systemTeal, label: "scroll \(dy)")

    case "drag":
        guard args.count >= 6,
              let x1 = Double(args[2]), let y1 = Double(args[3]),
              let x2 = Double(args[4]), let y2 = Double(args[5]) else {
            NSApplication.shared.terminate(nil); return
        }
        showDrag(x1: CGFloat(x1), y1: CGFloat(y1), x2: CGFloat(x2), y2: CGFloat(y2))

    case "move":
        guard args.count >= 4, let x = Double(args[2]), let y = Double(args[3]) else {
            NSApplication.shared.terminate(nil); return
        }
        showClick(x: CGFloat(x), y: CGFloat(y), color: .systemGray)

    case "screenshot":
        showScreenFlash()

    default:
        NSApplication.shared.terminate(nil)
    }
}

app.run()
