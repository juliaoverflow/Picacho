import Foundation
import SwiftUI

/// Decodes `widget-snapshot.json` written by Picacho (`src/manage/widgets.rs`)
/// into the shared App Group container.
struct WidgetSnapshot: Codable, Sendable {
    let version: Int
    let generatedAt: Int64
    let accentKey: String
    let accentLight: UInt32
    let accentDark: UInt32
    let accentSoftLight: UInt32
    let accentSoftDark: UInt32
    let totalUnread: Int
    let accounts: [AccountSummary]
    let messages: [WidgetMessage]
    let calendarConnected: Bool
    let events: [WidgetEvent]
    let packages: [WidgetPackage]
    let receipts: [WidgetReceipt]

    enum CodingKeys: String, CodingKey {
        case version
        case generatedAt = "generated_at"
        case accentKey = "accent_key"
        case accentLight = "accent_light"
        case accentDark = "accent_dark"
        case accentSoftLight = "accent_soft_light"
        case accentSoftDark = "accent_soft_dark"
        case totalUnread = "total_unread"
        case accounts
        case messages
        case calendarConnected = "calendar_connected"
        case events
        case packages
        case receipts
    }

    init(
        version: Int,
        generatedAt: Int64,
        accentKey: String,
        accentLight: UInt32,
        accentDark: UInt32,
        accentSoftLight: UInt32,
        accentSoftDark: UInt32,
        totalUnread: Int,
        accounts: [AccountSummary],
        messages: [WidgetMessage],
        calendarConnected: Bool,
        events: [WidgetEvent],
        packages: [WidgetPackage] = [],
        receipts: [WidgetReceipt] = []
    ) {
        self.version = version
        self.generatedAt = generatedAt
        self.accentKey = accentKey
        self.accentLight = accentLight
        self.accentDark = accentDark
        self.accentSoftLight = accentSoftLight
        self.accentSoftDark = accentSoftDark
        self.totalUnread = totalUnread
        self.accounts = accounts
        self.messages = messages
        self.calendarConnected = calendarConnected
        self.events = events
        self.packages = packages
        self.receipts = receipts
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        generatedAt = try c.decodeIfPresent(Int64.self, forKey: .generatedAt) ?? 0
        accentKey = try c.decodeIfPresent(String.self, forKey: .accentKey) ?? "green"
        accentLight = try c.decodeIfPresent(UInt32.self, forKey: .accentLight) ?? 0x246846
        accentDark = try c.decodeIfPresent(UInt32.self, forKey: .accentDark) ?? 0x86cfaa
        accentSoftLight = try c.decodeIfPresent(UInt32.self, forKey: .accentSoftLight) ?? 0xdcebe2
        accentSoftDark = try c.decodeIfPresent(UInt32.self, forKey: .accentSoftDark) ?? 0x30463b
        totalUnread = try c.decodeIfPresent(Int.self, forKey: .totalUnread) ?? 0
        accounts = try c.decodeIfPresent([AccountSummary].self, forKey: .accounts) ?? []
        messages = try c.decodeIfPresent([WidgetMessage].self, forKey: .messages) ?? []
        calendarConnected = try c.decodeIfPresent(Bool.self, forKey: .calendarConnected) ?? false
        events = try c.decodeIfPresent([WidgetEvent].self, forKey: .events) ?? []
        packages = try c.decodeIfPresent([WidgetPackage].self, forKey: .packages) ?? []
        receipts = try c.decodeIfPresent([WidgetReceipt].self, forKey: .receipts) ?? []
    }

    static let empty = WidgetSnapshot(
        version: 1,
        generatedAt: 0,
        accentKey: "green",
        accentLight: 0x246846,
        accentDark: 0x86cfaa,
        accentSoftLight: 0xdcebe2,
        accentSoftDark: 0x30463b,
        totalUnread: 0,
        accounts: [],
        messages: [],
        calendarConnected: false,
        events: []
    )

    static let placeholder = WidgetSnapshot(
        version: 1,
        generatedAt: Int64(Date().timeIntervalSince1970),
        accentKey: "green",
        accentLight: 0x246846,
        accentDark: 0x86cfaa,
        accentSoftLight: 0xdcebe2,
        accentSoftDark: 0x30463b,
        totalUnread: 3,
        accounts: [
            AccountSummary(id: "sample", name: "Inbox", email: "you@example.com", unread: 3)
        ],
        messages: [
            WidgetMessage(
                id: "1",
                accountId: "sample",
                accountLabel: "Inbox",
                sender: "Ana López",
                subject: "Weekend plans",
                snippet: "Are we still on for Saturday morning?",
                date: Int64(Date().timeIntervalSince1970) - 600,
                unread: true,
                starred: false,
                hasAttachments: false
            ),
            WidgetMessage(
                id: "2",
                accountId: "sample",
                accountLabel: "Inbox",
                sender: "Design Team",
                subject: "Updated mockups attached",
                snippet: "Here is the revised layout for the release.",
                date: Int64(Date().timeIntervalSince1970) - 3600,
                unread: true,
                starred: true,
                hasAttachments: true
            ),
            WidgetMessage(
                id: "3",
                accountId: "sample",
                accountLabel: "Inbox",
                sender: "Sam Patel",
                subject: "Notes from today's call",
                snippet: "Thanks everyone — action items below.",
                date: Int64(Date().timeIntervalSince1970) - 9200,
                unread: false,
                starred: false,
                hasAttachments: false
            ),
        ],
        calendarConnected: true,
        events: [
            WidgetEvent(
                uid: "evt-1",
                title: "Design review",
                location: "Room 4B",
                start: Int64(Date().timeIntervalSince1970) + 1800,
                end: Int64(Date().timeIntervalSince1970) + 5400,
                allDay: false,
                calendarName: "Work",
                calendarColor: "#246846"
            ),
            WidgetEvent(
                uid: "evt-2",
                title: "Coffee with Ana",
                location: "Downtown Cafe",
                start: Int64(Date().timeIntervalSince1970) + 10800,
                end: Int64(Date().timeIntervalSince1970) + 14400,
                allDay: false,
                calendarName: "Personal",
                calendarColor: "#2459A8"
            ),
        ],
        packages: [
            WidgetPackage(
                key: "1Z999AA10123456784",
                title: "Camera Lens",
                number: "1Z999AA10123456784",
                carrier: "UPS",
                status: "Out for delivery",
                stage: 3,
                delivered: false,
                expected: "Today",
                updated: Int64(Date().timeIntervalSince1970) - 1800,
                accountId: "sample",
                accountLabel: "Inbox",
                mailId: "1"
            ),
            WidgetPackage(
                key: "order:402-8821",
                title: "Order 402-8821",
                number: "",
                carrier: "USPS",
                status: "Delivered",
                stage: 4,
                delivered: true,
                expected: "",
                updated: Int64(Date().timeIntervalSince1970) - 86400,
                accountId: "sample",
                accountLabel: "Inbox",
                mailId: "2"
            ),
        ],
        receipts: [
            WidgetReceipt(
                key: "r1",
                merchant: "Desert Roasters",
                amount: "$18.40",
                refund: false,
                order: "9812",
                date: Int64(Date().timeIntervalSince1970) - 3600,
                updated: Int64(Date().timeIntervalSince1970) - 3600,
                accountId: "sample",
                accountLabel: "Inbox",
                mailId: "1"
            ),
            WidgetReceipt(
                key: "r2",
                merchant: "Northwind Books",
                amount: "+$24.00",
                refund: true,
                order: "4410",
                date: Int64(Date().timeIntervalSince1970) - 90000,
                updated: Int64(Date().timeIntervalSince1970) - 90000,
                accountId: "sample",
                accountLabel: "Inbox",
                mailId: "2"
            ),
        ]
    )

    static func load() -> WidgetSnapshot {
        guard let url = snapshotURL(),
              let data = try? Data(contentsOf: url),
              let decoded = try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
        else {
            return .empty
        }
        return decoded
    }

    /// The App Group container's widget-snapshot.json, written by Picacho.
    /// The group is named in the extension's Info.plist (`PicachoAppGroup`:
    /// macOS, where it is team-prefixed); otherwise it is "group." and the
    /// app's bundle ID (iOS). A sandboxed extension can read nothing else.
    private static func snapshotURL() -> URL? {
        let named = (Bundle.main.object(forInfoDictionaryKey: "PicachoAppGroup") as? String)?
            .trimmingCharacters(in: .whitespaces)
        let groupId: String
        if let named, !named.isEmpty, !named.hasPrefix("$(") {
            groupId = named
        } else if let extBundle = Bundle.main.bundleIdentifier {
            let suffix = ".widgets"
            let appBundle = extBundle.hasSuffix(suffix)
                ? String(extBundle.dropLast(suffix.count))
                : extBundle
            groupId = "group.\(appBundle)"
        } else {
            return nil
        }
        guard let url = FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: groupId)?
            .appendingPathComponent("widget-snapshot.json"),
            FileManager.default.fileExists(atPath: url.path)
        else {
            return nil
        }
        return url
    }

    func accent(for scheme: ColorScheme) -> Color {
        Color(rgb24: scheme == .dark ? accentDark : accentLight)
    }

    func accentSoft(for scheme: ColorScheme) -> Color {
        Color(rgb24: scheme == .dark ? accentSoftDark : accentSoftLight)
    }

    func unread(for accountId: String?) -> Int {
        guard let accountId, !accountId.isEmpty else { return totalUnread }
        return accounts.first(where: { $0.id == accountId })?.unread ?? 0
    }

    func label(for accountId: String?) -> String {
        if let accountId, !accountId.isEmpty {
            return accounts.first(where: { $0.id == accountId })?.name ?? "Inbox"
        }
        if accounts.count == 1 {
            return accounts[0].name
        }
        return "All Inboxes"
    }

    func accountLabel(for accountId: String?) -> String {
        if let accountId, !accountId.isEmpty {
            return accounts.first(where: { $0.id == accountId })?.name ?? "All Accounts"
        }
        if accounts.count == 1 {
            return accounts[0].name
        }
        return "All Accounts"
    }

    func filteredMessages(accountId: String?, unreadOnly: Bool, limit: Int) -> [WidgetMessage] {
        let filtered = messages.filter { msg in
            if let accountId, !accountId.isEmpty, msg.accountId != accountId {
                return false
            }
            if unreadOnly && !msg.unread {
                return false
            }
            return true
        }
        return Array(filtered.prefix(limit))
    }

    func filteredPackages(accountId: String?, limit: Int) -> [WidgetPackage] {
        let filtered = packages.filter { pkg in
            if let accountId, !accountId.isEmpty, pkg.accountId != accountId {
                return false
            }
            return true
        }
        return Array(filtered.prefix(limit))
    }

    /// How many packages the Summary lists for one account, or for All
    /// Accounts when none is chosen: the widgets' "View all N".
    func packageTotal(accountId: String?) -> Int {
        let chosen = accounts.filter { accountId == nil || accountId!.isEmpty || $0.id == accountId }
        let counted = chosen.reduce(0) { $0 + ($1.packages ?? 0) }
        return max(counted, filteredPackages(accountId: accountId, limit: Int.max).count)
    }

    /// How many receipts the Summary lists for one account, or for all.
    func receiptTotal(accountId: String?) -> Int {
        let chosen = accounts.filter { accountId == nil || accountId!.isEmpty || $0.id == accountId }
        let counted = chosen.reduce(0) { $0 + ($1.receipts ?? 0) }
        return max(counted, filteredReceipts(accountId: accountId, limit: Int.max).count)
    }

    func filteredReceipts(accountId: String?, limit: Int) -> [WidgetReceipt] {
        let filtered = receipts.filter { rcpt in
            if let accountId, !accountId.isEmpty, rcpt.accountId != accountId {
                return false
            }
            return true
        }
        return Array(filtered.prefix(limit))
    }

    func upcomingEvents(asOf now: Date, limit: Int) -> [WidgetEvent] {
        let nowTs = Int64(now.timeIntervalSince1970)
        var utcCal = Calendar(identifier: .gregorian)
        utcCal.timeZone = TimeZone(secondsFromGMT: 0)!
        let localComps = Calendar.current.dateComponents([.year, .month, .day], from: now)
        let todayUtc = utcCal.date(from: localComps).map { Int64($0.timeIntervalSince1970) } ?? (nowTs - 86_400)

        let filtered = events.filter { ev in
            if ev.allDay {
                let effectiveEnd = ev.end > ev.start ? ev.end : ev.start + 86_400
                return effectiveEnd > todayUtc
            } else {
                return max(ev.end, ev.start + 60) > nowTs
            }
        }
        return Array(filtered.prefix(limit))
    }
}

struct AccountSummary: Codable, Hashable, Identifiable, Sendable {
    let id: String
    let name: String
    let email: String
    let unread: Int
    /// Packages and receipts the Summary lists for this account (absent in
    /// snapshots from before they were counted).
    var packages: Int? = nil
    var receipts: Int? = nil
}

struct WidgetMessage: Codable, Hashable, Identifiable, Sendable {
    let id: String
    let accountId: String
    let accountLabel: String
    let sender: String
    let subject: String
    let snippet: String
    let date: Int64
    let unread: Bool
    let starred: Bool
    let hasAttachments: Bool

    enum CodingKeys: String, CodingKey {
        case id
        case accountId = "account_id"
        case accountLabel = "account_label"
        case sender
        case subject
        case snippet
        case date
        case unread
        case starred
        case hasAttachments = "has_attachments"
    }
}

struct WidgetEvent: Codable, Hashable, Identifiable, Sendable {
    let uid: String
    let title: String
    let location: String
    let start: Int64
    let end: Int64
    let allDay: Bool
    let calendarName: String
    let calendarColor: String

    var id: String { "\(uid)@\(start)" }

    enum CodingKeys: String, CodingKey {
        case uid
        case title
        case location
        case start
        case end
        case allDay = "all_day"
        case calendarName = "calendar_name"
        case calendarColor = "calendar_color"
    }
}

struct WidgetPackage: Codable, Hashable, Identifiable, Sendable {
    let key: String
    let title: String
    let number: String
    let carrier: String
    let status: String
    let stage: Int
    let delivered: Bool
    let expected: String
    let updated: Int64
    let accountId: String
    let accountLabel: String
    let mailId: String

    var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, title, number, carrier, status, stage, delivered, expected, updated
        case accountId = "account_id"
        case accountLabel = "account_label"
        case mailId = "mail_id"
    }

    init(
        key: String,
        title: String,
        number: String,
        carrier: String,
        status: String,
        stage: Int,
        delivered: Bool,
        expected: String,
        updated: Int64,
        accountId: String,
        accountLabel: String = "",
        mailId: String
    ) {
        self.key = key
        self.title = title
        self.number = number
        self.carrier = carrier
        self.status = status
        self.stage = stage
        self.delivered = delivered
        self.expected = expected
        self.updated = updated
        self.accountId = accountId
        self.accountLabel = accountLabel
        self.mailId = mailId
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        number = try c.decodeIfPresent(String.self, forKey: .number) ?? ""
        carrier = try c.decodeIfPresent(String.self, forKey: .carrier) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        stage = try c.decodeIfPresent(Int.self, forKey: .stage) ?? 0
        delivered = try c.decodeIfPresent(Bool.self, forKey: .delivered) ?? false
        expected = try c.decodeIfPresent(String.self, forKey: .expected) ?? ""
        updated = try c.decodeIfPresent(Int64.self, forKey: .updated) ?? 0
        accountId = try c.decodeIfPresent(String.self, forKey: .accountId) ?? ""
        accountLabel = try c.decodeIfPresent(String.self, forKey: .accountLabel) ?? ""
        mailId = try c.decodeIfPresent(String.self, forKey: .mailId) ?? ""
    }
}

struct WidgetReceipt: Codable, Hashable, Identifiable, Sendable {
    let key: String
    let merchant: String
    let amount: String
    let refund: Bool
    let order: String
    let date: Int64
    let updated: Int64
    let accountId: String
    let accountLabel: String
    let mailId: String

    var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, merchant, amount, refund, order, date, updated
        case accountId = "account_id"
        case accountLabel = "account_label"
        case mailId = "mail_id"
    }

    init(
        key: String,
        merchant: String,
        amount: String,
        refund: Bool,
        order: String,
        date: Int64,
        updated: Int64 = 0,
        accountId: String,
        accountLabel: String = "",
        mailId: String
    ) {
        self.key = key
        self.merchant = merchant
        self.amount = amount
        self.refund = refund
        self.order = order
        self.date = date
        self.updated = updated > 0 ? updated : date
        self.accountId = accountId
        self.accountLabel = accountLabel
        self.mailId = mailId
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
        merchant = try c.decodeIfPresent(String.self, forKey: .merchant) ?? ""
        amount = try c.decodeIfPresent(String.self, forKey: .amount) ?? ""
        refund = try c.decodeIfPresent(Bool.self, forKey: .refund) ?? false
        order = try c.decodeIfPresent(String.self, forKey: .order) ?? ""
        date = try c.decodeIfPresent(Int64.self, forKey: .date) ?? 0
        let rawUpdated = try c.decodeIfPresent(Int64.self, forKey: .updated) ?? 0
        updated = rawUpdated > 0 ? rawUpdated : date
        accountId = try c.decodeIfPresent(String.self, forKey: .accountId) ?? ""
        accountLabel = try c.decodeIfPresent(String.self, forKey: .accountLabel) ?? ""
        mailId = try c.decodeIfPresent(String.self, forKey: .mailId) ?? ""
    }
}

enum WidgetLinks {
    static func inbox(accountId: String?) -> URL {
        var components = URLComponents(string: "picacho://widget/inbox")!
        if let accountId, !accountId.isEmpty {
            components.queryItems = [URLQueryItem(name: "account", value: accountId)]
        }
        return components.url!
    }

    static func compose(accountId: String?) -> URL {
        var components = URLComponents(string: "picacho://widget/compose")!
        if let accountId, !accountId.isEmpty {
            components.queryItems = [URLQueryItem(name: "account", value: accountId)]
        }
        return components.url!
    }

    static func message(accountId: String, id: String) -> URL {
        var components = URLComponents(string: "picacho://widget/message")!
        components.queryItems = [
            URLQueryItem(name: "account", value: accountId),
            URLQueryItem(name: "id", value: id),
        ]
        return components.url!
    }

    static var calendar: URL {
        URL(string: "picacho://widget/calendar")!
    }

    static func packages(accountId: String?) -> URL {
        var components = URLComponents(string: "picacho://widget/packages")!
        if let accountId, !accountId.isEmpty {
            components.queryItems = [URLQueryItem(name: "account", value: accountId)]
        }
        return components.url!
    }

    static func receipts(accountId: String?) -> URL {
        var components = URLComponents(string: "picacho://widget/receipts")!
        if let accountId, !accountId.isEmpty {
            components.queryItems = [URLQueryItem(name: "account", value: accountId)]
        }
        return components.url!
    }
}

extension Color {
    init(rgb24: UInt32) {
        let r = Double((rgb24 >> 16) & 0xFF) / 255.0
        let g = Double((rgb24 >> 8) & 0xFF) / 255.0
        let b = Double(rgb24 & 0xFF) / 255.0
        self.init(red: r, green: g, blue: b)
    }

    init?(hex: String) {
        let trimmed = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("#"), trimmed.count == 7,
              let value = UInt32(trimmed.dropFirst(), radix: 16)
        else {
            return nil
        }
        self.init(rgb24: value)
    }
}

enum WidgetFormatting {
    static func messageDate(_ epochSeconds: Int64) -> String {
        guard epochSeconds > 0 else { return "" }
        let date = Date(timeIntervalSince1970: TimeInterval(epochSeconds))
        let cal = Calendar.current
        if cal.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
        }
        if let days = cal.dateComponents([.day], from: date, to: Date()).day, days >= 0, days < 6 {
            return date.formatted(.dateTime.weekday(.abbreviated))
        }
        if cal.component(.year, from: date) == cal.component(.year, from: Date()) {
            return date.formatted(.dateTime.month(.abbreviated).day())
        }
        return date.formatted(date: .abbreviated, time: .omitted)
    }

    static func eventTime(_ event: WidgetEvent) -> String {
        let cal = Calendar.current
        if event.allDay {
            var utc = Calendar(identifier: .gregorian)
            utc.timeZone = TimeZone(secondsFromGMT: 0)!
            let date = Date(timeIntervalSince1970: TimeInterval(event.start))
            let comps = utc.dateComponents([.year, .month, .day], from: date)
            let localDate = cal.date(from: comps) ?? date
            return "\(dayLabel(localDate)) · All day"
        }
        let start = Date(timeIntervalSince1970: TimeInterval(event.start))
        let day = dayLabel(start)
        let startStr = start.formatted(date: .omitted, time: .shortened)
        if event.end > event.start {
            let end = Date(timeIntervalSince1970: TimeInterval(event.end))
            let endStr = end.formatted(date: .omitted, time: .shortened)
            return "\(day) · \(startStr) – \(endStr)"
        }
        return "\(day) · \(startStr)"
    }

    private static func dayLabel(_ date: Date) -> String {
        let cal = Calendar.current
        if cal.isDateInToday(date) {
            return "Today"
        }
        if cal.isDateInTomorrow(date) {
            return "Tomorrow"
        }
        return date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())
    }
}
