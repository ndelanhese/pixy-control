// Event-driven USB observer. No polling, camera capture, image processing or
// network. launchd starts it at login; an already attached Pixy counts as arrival.
import AppKit
import IOKit

let bundleID = "dev.ndelanhese.pixy-control"
guard CommandLine.arguments.count == 2 else { exit(64) }
let appURL = URL(fileURLWithPath: CommandLine.arguments[1])

func attached(_ context: UnsafeMutableRawPointer?, _ iterator: io_iterator_t) {
    var found = false
    while case let service = IOIteratorNext(iterator), service != 0 {
        found = true
        IOObjectRelease(service)
    }
    guard found else { return }
    // Never activate/reopen an existing process, including after explicit Quit.
    guard NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).isEmpty else { return }
    let config = NSWorkspace.OpenConfiguration()
    config.activates = false
    config.hides = true
    config.arguments = ["--background"]
    NSWorkspace.shared.openApplication(at: appURL, configuration: config) { _, error in
        if let error { fputs("Pixy device launch: \(error)\n", stderr) }
    }
}

guard let port = IONotificationPortCreate(kIOMainPortDefault),
      let source = IONotificationPortGetRunLoopSource(port)?.takeUnretainedValue(),
      let matching = IOServiceMatching("IOUSBHostDevice") else { exit(1) }
let dictionary = matching as NSMutableDictionary
dictionary["idVendor"] = 0x328f
dictionary["idProduct"] = 0x00c0
var iterator: io_iterator_t = 0
CFRunLoopAddSource(CFRunLoopGetMain(), source, .defaultMode)
let result = IOServiceAddMatchingNotification(port, kIOFirstMatchNotification,
    dictionary, attached, nil, &iterator)
guard result == KERN_SUCCESS else { exit(1) }
attached(nil, iterator) // Drain initial devices and arm future notifications.
CFRunLoopRun()
