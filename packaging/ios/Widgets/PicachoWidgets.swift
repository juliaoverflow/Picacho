// Picacho's WidgetKit widgets (iOS and macOS). Senders, subjects, events,
// packages and receipts are marked `.privacySensitive()`: on a locked device
// (the Lock Screen, StandBy) and in redacted previews the system hides them.
import AppIntents
import SwiftUI
import WidgetKit

// MARK: - Configurable account choice (iOS 17 AppIntents)

struct AccountOption: AppEntity {
    let id: String
    let name: String

    static var typeDisplayRepresentation: TypeDisplayRepresentation = "Account"
    static var defaultQuery = AccountOptionQuery()

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)")
    }

    static let all = AccountOption(id: "", name: String(localized: "All Accounts", bundle: .picacho, comment: "The choice of every account in a widget's settings"))
}

struct AccountOptionQuery: EntityQuery {
    func entities(for identifiers: [String]) async throws -> [AccountOption] {
        let available = allOptions()
        return identifiers.compactMap { id in available.first(where: { $0.id == id }) }
    }

    func suggestedEntities() async throws -> [AccountOption] {
        allOptions()
    }

    func defaultResult() async -> AccountOption? {
        .all
    }

    private func allOptions() -> [AccountOption] {
        let snap = WidgetSnapshot.load()
        var options: [AccountOption] = [.all]
        for account in snap.accounts {
            let label = account.name.isEmpty || account.name == account.email
                ? account.email
                : String(localized: "\(account.name) (\(account.email))", bundle: .picacho, comment: "An account in a widget's settings: its name, then its address")
            options.append(AccountOption(id: account.id, name: label))
        }
        return options
    }
}

struct InboxWidgetIntent: WidgetConfigurationIntent {
    static var title: LocalizedStringResource = "Inbox"
    static var description = IntentDescription("Choose an account and whether to show only unread messages.")

    @Parameter(title: "Account")
    var account: AccountOption?

    @Parameter(title: "Unread Only", default: false)
    var unreadOnly: Bool
}

struct UnreadWidgetIntent: WidgetConfigurationIntent {
    static var title: LocalizedStringResource = "Unread Mail"
    static var description = IntentDescription("Choose which account's unread count to display.")

    @Parameter(title: "Account")
    var account: AccountOption?
}

struct PackagesWidgetIntent: WidgetConfigurationIntent {
    static var title: LocalizedStringResource = "Packages"
    static var description = IntentDescription("Choose an account or All Accounts for recent packages.")

    @Parameter(title: "Account")
    var account: AccountOption?
}

struct ReceiptsWidgetIntent: WidgetConfigurationIntent {
    static var title: LocalizedStringResource = "Receipts"
    static var description = IntentDescription("Choose an account or All Accounts for recent receipts.")

    @Parameter(title: "Account")
    var account: AccountOption?
}

// MARK: - Timeline entries & providers

struct InboxEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot
    let accountId: String?
    let unreadOnly: Bool
}

struct InboxProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> InboxEntry {
        InboxEntry(date: Date(), snapshot: .placeholder, accountId: nil, unreadOnly: false)
    }

    func snapshot(for configuration: InboxWidgetIntent, in context: Context) async -> InboxEntry {
        let snap = context.isPreview ? WidgetSnapshot.placeholder : WidgetSnapshot.load()
        return InboxEntry(
            date: Date(),
            snapshot: snap,
            accountId: configuration.account?.id,
            unreadOnly: configuration.unreadOnly
        )
    }

    func timeline(for configuration: InboxWidgetIntent, in context: Context) async -> Timeline<InboxEntry> {
        let now = Date()
        let entry = InboxEntry(
            date: now,
            snapshot: WidgetSnapshot.load(),
            accountId: configuration.account?.id,
            unreadOnly: configuration.unreadOnly
        )
        let nextRefresh = Calendar.current.date(byAdding: .minute, value: 30, to: now) ?? now.addingTimeInterval(1800)
        return Timeline(entries: [entry], policy: .after(nextRefresh))
    }
}

struct UnreadProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> InboxEntry {
        InboxEntry(date: Date(), snapshot: .placeholder, accountId: nil, unreadOnly: true)
    }

    func snapshot(for configuration: UnreadWidgetIntent, in context: Context) async -> InboxEntry {
        let snap = context.isPreview ? WidgetSnapshot.placeholder : WidgetSnapshot.load()
        return InboxEntry(date: Date(), snapshot: snap, accountId: configuration.account?.id, unreadOnly: true)
    }

    func timeline(for configuration: UnreadWidgetIntent, in context: Context) async -> Timeline<InboxEntry> {
        let now = Date()
        let entry = InboxEntry(
            date: now,
            snapshot: WidgetSnapshot.load(),
            accountId: configuration.account?.id,
            unreadOnly: true
        )
        let nextRefresh = Calendar.current.date(byAdding: .minute, value: 30, to: now) ?? now.addingTimeInterval(1800)
        return Timeline(entries: [entry], policy: .after(nextRefresh))
    }
}

struct CalendarEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot
}

struct CalendarProvider: TimelineProvider {
    func placeholder(in context: Context) -> CalendarEntry {
        CalendarEntry(date: Date(), snapshot: .placeholder)
    }

    func getSnapshot(in context: Context, completion: @escaping (CalendarEntry) -> Void) {
        let snap = context.isPreview ? WidgetSnapshot.placeholder : WidgetSnapshot.load()
        completion(CalendarEntry(date: Date(), snapshot: snap))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<CalendarEntry>) -> Void) {
        let now = Date()
        let snap = WidgetSnapshot.load()
        let entry = CalendarEntry(date: now, snapshot: snap)

        // Refresh at the next upcoming event end (so finished events drop off),
        // or at midnight so "Today" / "Tomorrow" labels roll over accurately.
        let midnight = Calendar.current.nextDate(
            after: now,
            matching: DateComponents(hour: 0, minute: 0, second: 5),
            matchingPolicy: .nextTime
        ) ?? now.addingTimeInterval(3600)

        let nextEventEnd = snap.upcomingEvents(asOf: now, limit: 1).first.flatMap { ev -> Date? in
            guard !ev.allDay else { return nil }
            let end = Date(timeIntervalSince1970: TimeInterval(max(ev.end, ev.start + 60)))
            return end > now ? end.addingTimeInterval(5) : nil
        }

        let refreshDate = min(nextEventEnd ?? midnight, midnight)
        completion(Timeline(entries: [entry], policy: .after(refreshDate)))
    }
}

// MARK: - Inbox Widget

struct InboxWidget: Widget {
    static let kind = "PicachoInboxWidget"

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: Self.kind, intent: InboxWidgetIntent.self, provider: InboxProvider()) { entry in
            InboxWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Inbox")
        .description("Recent inbox messages, unread count, and quick compose.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

struct InboxWidgetView: View {
    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var colorScheme
    let entry: InboxEntry

    private var rowLimit: Int {
        switch family {
        case .systemSmall: return 2
        case .systemMedium: return 3
        case .systemLarge: return 6
        default: return 3
        }
    }

    var body: some View {
        let snap = entry.snapshot
        let accent = snap.accent(for: colorScheme)
        let unread = snap.unread(for: entry.accountId)
        let messages = snap.filteredMessages(accountId: entry.accountId, unreadOnly: entry.unreadOnly, limit: rowLimit)
        let label = snap.label(for: entry.accountId)
        let title = entry.unreadOnly
            ? String(localized: "\(label) · Unread", bundle: .picacho, comment: "The Inbox widget's title when it shows only unread messages: the account, or All Inboxes")
            : label

        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "envelope.fill")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(accent)
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(1)
                if unread > 0 {
                    Text(unread > 999 ? "999+" : "\(unread)")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(accent)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(snap.accentSoft(for: colorScheme), in: Capsule())
                }
                Spacer(minLength: 4)
                if family != .systemSmall {
                    Link(destination: WidgetLinks.compose(accountId: entry.accountId)) {
                        Image(systemName: "square.and.pencil")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(accent)
                            .frame(width: 26, height: 26)
                            .background(snap.accentSoft(for: colorScheme), in: Circle())
                    }
                }
            }

            Divider()

            if messages.isEmpty {
                Spacer()
                Text(emptyText(snap: snap))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                Spacer()
            } else {
                VStack(alignment: .leading, spacing: family == .systemSmall ? 6 : 7) {
                    ForEach(messages) { msg in
                        if family == .systemSmall {
                            messageRow(msg, accent: accent, compact: true)
                        } else {
                            Link(destination: WidgetLinks.message(accountId: msg.accountId, id: msg.id)) {
                                messageRow(msg, accent: accent, compact: false)
                            }
                        }
                    }
                }
                Spacer(minLength: 0)
            }
        }
        .widgetURL(WidgetLinks.inbox(accountId: entry.accountId))
    }

    private func emptyText(snap: WidgetSnapshot) -> String {
        if snap.accounts.isEmpty { return String(localized: "Open Picacho to sign in", bundle: .picacho) }
        return entry.unreadOnly
            ? String(localized: "All caught up", bundle: .picacho, comment: "No unread messages")
            : String(localized: "No messages in Inbox", bundle: .picacho)
    }

    private func messageRow(_ msg: WidgetMessage, accent: Color, compact: Bool) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Circle()
                .fill(msg.unread ? accent : Color.clear)
                .frame(width: 6, height: 6)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    Text(msg.sender.isEmpty ? String(localized: "(Unknown sender)", bundle: .picacho) : msg.sender)
                        .privacySensitive()
                        .font(.system(size: 12, weight: msg.unread ? .semibold : .regular))
                        .foregroundStyle(msg.unread ? .primary : .secondary)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if !compact {
                        if msg.hasAttachments {
                            Image(systemName: "paperclip")
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                        if msg.starred {
                            Image(systemName: "star.fill")
                                .font(.system(size: 10))
                                .foregroundStyle(.orange)
                        }
                    }
                    Text(WidgetFormatting.messageDate(msg.date))
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
                Text(msg.subject.isEmpty ? String(localized: "(No subject)", bundle: .picacho) : msg.subject)
                    .privacySensitive()
                    .font(.system(size: 12, weight: msg.unread ? .medium : .regular))
                    .foregroundStyle(msg.unread ? .primary : .secondary)
                    .lineLimit(1)
                if !compact && !msg.snippet.isEmpty {
                    Text(msg.snippet)
                        .privacySensitive()
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
    }
}

// MARK: - Unread Count / Quick Mail Widget

struct UnreadWidget: Widget {
    static let kind = "PicachoUnreadWidget"

    private static var families: [WidgetFamily] {
        #if os(iOS)
        return [.systemSmall, .accessoryCircular, .accessoryRectangular]
        #else
        return [.systemSmall]
        #endif
    }

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: Self.kind, intent: UnreadWidgetIntent.self, provider: UnreadProvider()) { entry in
            UnreadWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Unread Mail")
        .description("Unread count, latest message, and quick compose.")
        .supportedFamilies(Self.families)
    }
}

struct UnreadWidgetView: View {
    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var colorScheme
    let entry: InboxEntry

    var body: some View {
        let snap = entry.snapshot
        let accent = snap.accent(for: colorScheme)
        let unread = snap.unread(for: entry.accountId)
        let latest = snap.filteredMessages(accountId: entry.accountId, unreadOnly: true, limit: 1).first

        #if os(iOS)
        switch family {
        case .accessoryCircular:
            ZStack {
                AccessoryWidgetBackground()
                VStack(spacing: 1) {
                    Image(systemName: "envelope.fill")
                        .font(.system(size: 12))
                    Text(unread > 99 ? "99+" : "\(unread)")
                        .font(.system(size: 16, weight: .bold, design: .rounded))
                }
            }
            .widgetURL(WidgetLinks.inbox(accountId: entry.accountId))

        case .accessoryRectangular:
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Image(systemName: "envelope.fill")
                        .font(.system(size: 11, weight: .semibold))
                    Text(unread == 0
                         ? String(localized: "All caught up", bundle: .picacho, comment: "No unread messages")
                         : String(localized: "\(unread) unread", bundle: .picacho, comment: "How many unread messages there are"))
                        .font(.system(size: 12, weight: .semibold))
                }
                if let latest {
                    Text(latest.sender)
                        .privacySensitive()
                        .font(.system(size: 11, weight: .medium))
                        .lineLimit(1)
                    Text(latest.subject.isEmpty ? String(localized: "(No subject)", bundle: .picacho) : latest.subject)
                        .privacySensitive()
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else {
                    Text(snap.label(for: entry.accountId))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            .widgetURL(WidgetLinks.inbox(accountId: entry.accountId))

        default:
            smallBody(snap: snap, accent: accent, unread: unread, latest: latest)
        }
        #else
        smallBody(snap: snap, accent: accent, unread: unread, latest: latest)
        #endif
    }

    @ViewBuilder
    private func smallBody(snap: WidgetSnapshot, accent: Color, unread: Int, latest: WidgetMessage?) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Image(systemName: "envelope.fill")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(accent)
                Spacer()
                Link(destination: WidgetLinks.compose(accountId: entry.accountId)) {
                    Image(systemName: "square.and.pencil")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(accent)
                        .frame(width: 28, height: 28)
                        .background(snap.accentSoft(for: colorScheme), in: Circle())
                }
            }

            Spacer(minLength: 2)

            HStack(alignment: .firstTextBaseline, spacing: 5) {
                Text(unread > 999 ? "999+" : "\(unread)")
                    .font(.system(size: 32, weight: .bold, design: .rounded))
                    .foregroundStyle(unread > 0 ? accent : .primary)
                Text(unread == 0
                     ? String(localized: "Caught up", bundle: .picacho, comment: "Under an unread count of 0, shown larger beside it")
                     : String(localized: "unread", bundle: .picacho, comment: "Beside the unread count, which is shown larger"))
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }

            if let latest {
                Text(latest.sender)
                    .privacySensitive()
                    .font(.system(size: 11, weight: .semibold))
                    .lineLimit(1)
                Text(latest.subject.isEmpty ? String(localized: "(No subject)", bundle: .picacho) : latest.subject)
                    .privacySensitive()
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            } else {
                Text(snap.label(for: entry.accountId))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .widgetURL(WidgetLinks.inbox(accountId: entry.accountId))
    }
}

// MARK: - Calendar Agenda Widget

struct CalendarWidget: Widget {
    static let kind = "PicachoCalendarWidget"

    private static var families: [WidgetFamily] {
        #if os(iOS)
        return [.systemSmall, .systemMedium, .systemLarge, .accessoryRectangular]
        #else
        return [.systemSmall, .systemMedium, .systemLarge]
        #endif
    }

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: Self.kind, provider: CalendarProvider()) { entry in
            CalendarWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Calendar")
        .description("Upcoming events from your Picacho calendars.")
        .supportedFamilies(Self.families)
    }
}

struct CalendarWidgetView: View {
    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var colorScheme
    let entry: CalendarEntry

    private var eventLimit: Int {
        switch family {
        case .systemSmall: return 2
        case .systemMedium: return 3
        case .systemLarge: return 7
        default: return 2
        }
    }

    var body: some View {
        let snap = entry.snapshot
        let accent = snap.accent(for: colorScheme)
        let events = snap.upcomingEvents(asOf: entry.date, limit: eventLimit)

        #if os(iOS)
        if family == .accessoryRectangular {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Image(systemName: "calendar")
                        .font(.system(size: 11, weight: .semibold))
                    Text(entry.date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day().locale(PicachoLanguage.locale)))
                        .font(.system(size: 11, weight: .semibold))
                }
                if let first = events.first {
                    Text(first.title)
                        .privacySensitive()
                        .font(.system(size: 12, weight: .semibold))
                        .lineLimit(1)
                    Text(WidgetFormatting.eventTime(first))
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                } else {
                    Text(String(localized: "No upcoming events", bundle: .picacho))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
            }
            .widgetURL(WidgetLinks.calendar)
        } else {
            standardBody(snap: snap, accent: accent, events: events)
        }
        #else
        standardBody(snap: snap, accent: accent, events: events)
        #endif
    }

    @ViewBuilder
    private func standardBody(snap: WidgetSnapshot, accent: Color, events: [WidgetEvent]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "calendar")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(accent)
                Text(String(localized: "Calendar", bundle: .picacho))
                    .font(.system(size: 14, weight: .semibold))
                Spacer(minLength: 4)
                Text(entry.date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day().locale(PicachoLanguage.locale)))
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            Divider()

            if events.isEmpty {
                Spacer()
                Text(snap.calendarConnected
                     ? String(localized: "No upcoming events", bundle: .picacho)
                     : String(localized: "Connect a calendar in Settings", bundle: .picacho))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                Spacer()
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(events) { ev in
                        HStack(spacing: 8) {
                            RoundedRectangle(cornerRadius: 1.5)
                                .fill(Color(hex: ev.calendarColor) ?? accent)
                                .frame(width: 3, height: ev.location.isEmpty || family == .systemSmall ? 28 : 38)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(ev.title)
                                    .privacySensitive()
                                    .font(.system(size: 12, weight: .semibold))
                                    .lineLimit(1)
                                Text(WidgetFormatting.eventTime(ev))
                                    .font(.system(size: 11))
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                if family != .systemSmall && !ev.location.isEmpty {
                                    Text(ev.location)
                                        .privacySensitive()
                                        .font(.system(size: 10))
                                        .foregroundStyle(.tertiary)
                                        .lineLimit(1)
                                }
                            }
                            Spacer(minLength: 0)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
        }
        .widgetURL(WidgetLinks.calendar)
    }
}

// MARK: - Packages Widget

struct SummaryEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot
    let accountId: String?
}

struct PackagesProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> SummaryEntry {
        SummaryEntry(date: Date(), snapshot: .placeholder, accountId: nil)
    }

    func snapshot(for configuration: PackagesWidgetIntent, in context: Context) async -> SummaryEntry {
        let snap = context.isPreview ? WidgetSnapshot.placeholder : WidgetSnapshot.load()
        return SummaryEntry(date: Date(), snapshot: snap, accountId: configuration.account?.id)
    }

    func timeline(for configuration: PackagesWidgetIntent, in context: Context) async -> Timeline<SummaryEntry> {
        let now = Date()
        let entry = SummaryEntry(date: now, snapshot: WidgetSnapshot.load(), accountId: configuration.account?.id)
        let next = Calendar.current.date(byAdding: .minute, value: 30, to: now) ?? now.addingTimeInterval(1800)
        return Timeline(entries: [entry], policy: .after(next))
    }
}

struct ReceiptsProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> SummaryEntry {
        SummaryEntry(date: Date(), snapshot: .placeholder, accountId: nil)
    }

    func snapshot(for configuration: ReceiptsWidgetIntent, in context: Context) async -> SummaryEntry {
        let snap = context.isPreview ? WidgetSnapshot.placeholder : WidgetSnapshot.load()
        return SummaryEntry(date: Date(), snapshot: snap, accountId: configuration.account?.id)
    }

    func timeline(for configuration: ReceiptsWidgetIntent, in context: Context) async -> Timeline<SummaryEntry> {
        let now = Date()
        let entry = SummaryEntry(date: now, snapshot: WidgetSnapshot.load(), accountId: configuration.account?.id)
        let next = Calendar.current.date(byAdding: .minute, value: 30, to: now) ?? now.addingTimeInterval(1800)
        return Timeline(entries: [entry], policy: .after(next))
    }
}

struct PackagesWidget: Widget {
    static let kind = "PicachoPackagesWidget"

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: Self.kind, intent: PackagesWidgetIntent.self, provider: PackagesProvider()) { entry in
            PackagesWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Packages")
        .description("Your most recently updated packages and delivery progress.")
        .supportedFamilies(summaryFamilies)
    }
}

struct PackagesWidgetView: View {
    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var colorScheme
    let entry: SummaryEntry

    private var compact: Bool { family == .systemSmall }
    /// A row's height: fixed, so the list shows as many whole rows as the
    /// widget's size fits (no scaling), and "View all" has the rest.
    private var rowHeight: CGFloat { compact ? 30 : 40 }
    private let rowSpacing: CGFloat = 6

    var body: some View {
        let snap = entry.snapshot
        let accent = snap.accent(for: colorScheme)
        let matching = snap.filteredPackages(accountId: entry.accountId, limit: Int.max)
        let total = snap.packageTotal(accountId: entry.accountId)
        let hasAccountFilter = !(entry.accountId ?? "").isEmpty
        let title = hasAccountFilter
            ? String(localized: "Packages · \(snap.accountLabel(for: entry.accountId))", bundle: .picacho, comment: "The Packages widget's title when it shows one account's")
            : String(localized: "Packages", bundle: .picacho)
        let showAccount = !hasAccountFilter && snap.accounts.count > 1

        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "shippingbox.fill")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(accent)
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(1)
                Spacer(minLength: 4)
                if total > 0 {
                    ViewAllLabel(total: total, accent: accent, compact: compact,
                                 link: WidgetLinks.packages(accountId: entry.accountId))
                }
            }

            Divider()

            if matching.isEmpty {
                Spacer()
                Text(String(localized: "No recent packages", bundle: .picacho))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                Spacer()
            } else {
                GeometryReader { geo in
                    let fit = max(0, Int((geo.size.height + rowSpacing) / (rowHeight + rowSpacing)))
                    VStack(alignment: .leading, spacing: rowSpacing) {
                        ForEach(Array(matching.prefix(fit))) { pkg in
                            if compact {
                                packageRow(pkg, accent: accent, compact: true, showAccount: false)
                                    .frame(height: rowHeight, alignment: .top)
                            } else {
                                Link(destination: WidgetLinks.message(accountId: pkg.accountId, id: pkg.mailId)) {
                                    packageRow(pkg, accent: accent, compact: false, showAccount: showAccount)
                                        .frame(height: rowHeight, alignment: .top)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }
            }
        }
        .widgetURL(WidgetLinks.packages(accountId: entry.accountId))
    }

    private func packageRow(_ pkg: WidgetPackage, accent: Color, compact: Bool, showAccount: Bool) -> some View {
        let subtitleParts: [String] = {
            var parts = [pkg.status]
            if !pkg.carrier.isEmpty { parts.append(pkg.carrier) }
            if !pkg.expected.isEmpty {
                parts.append(String(localized: "Expected \(pkg.expected)", bundle: .picacho, comment: "When a package should arrive, for example Expected Nov 26"))
            } else {
                let d = WidgetFormatting.messageDate(pkg.updated)
                if !d.isEmpty { parts.append(d) }
            }
            if showAccount && !pkg.accountLabel.isEmpty {
                parts.append(pkg.accountLabel)
            }
            return parts
        }()
        return HStack(alignment: .top, spacing: 7) {
            Image(systemName: pkg.delivered ? "checkmark.circle.fill" : "shippingbox")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(pkg.delivered ? .secondary : accent)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(pkg.title.isEmpty ? String(localized: "Package", bundle: .picacho, comment: "Stands in for a package without a name") : pkg.title)
                    .privacySensitive()
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                Text(subtitleParts.joined(separator: " · "))
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                if !compact {
                    HStack(spacing: 3) {
                        ForEach(0..<5, id: \.self) { step in
                            Capsule()
                                .fill(step <= pkg.stage ? (pkg.delivered ? Color.secondary.opacity(0.6) : accent) : Color.secondary.opacity(0.2))
                                .frame(height: 3)
                        }
                    }
                }
            }
        }
    }
}

/// The sizes the Packages and Receipts widgets come in: the larger, the more
/// rows they show.
private let summaryFamilies: [WidgetFamily] = [.systemSmall, .systemMedium, .systemLarge, .systemExtraLarge]

/// "View all N" in a Packages or Receipts widget's header, opening them in
/// the Summary. A small widget opens there as a whole (it takes no links).
struct ViewAllLabel: View {
    let total: Int
    let accent: Color
    let compact: Bool
    let link: URL

    var body: some View {
        let label = Text(String(localized: "View all \(total)", bundle: .picacho, comment: "Opens the whole list; the number is how many there are"))
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(accent)
            .lineLimit(1)
        if compact {
            label
        } else {
            Link(destination: link) { label }
        }
    }
}

// MARK: - Receipts Widget

struct ReceiptsWidget: Widget {
    static let kind = "PicachoReceiptsWidget"

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: Self.kind, intent: ReceiptsWidgetIntent.self, provider: ReceiptsProvider()) { entry in
            ReceiptsWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Receipts")
        .description("Your most recently updated receipts and refunds.")
        .supportedFamilies(summaryFamilies)
    }
}

struct ReceiptsWidgetView: View {
    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var colorScheme
    let entry: SummaryEntry

    private var compact: Bool { family == .systemSmall }
    /// A row's height: fixed, so the list shows as many whole rows as fit.
    private var rowHeight: CGFloat { compact ? 28 : 30 }
    private let rowSpacing: CGFloat = 6

    var body: some View {
        let snap = entry.snapshot
        let accent = snap.accent(for: colorScheme)
        let matching = snap.filteredReceipts(accountId: entry.accountId, limit: Int.max)
        let total = snap.receiptTotal(accountId: entry.accountId)
        let hasAccountFilter = !(entry.accountId ?? "").isEmpty
        let title = hasAccountFilter
            ? String(localized: "Receipts · \(snap.accountLabel(for: entry.accountId))", bundle: .picacho, comment: "The Receipts widget's title when it shows one account's")
            : String(localized: "Receipts", bundle: .picacho)
        let showAccount = !hasAccountFilter && snap.accounts.count > 1

        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "doc.text.fill")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(accent)
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .lineLimit(1)
                Spacer(minLength: 4)
                if total > 0 {
                    ViewAllLabel(total: total, accent: accent, compact: compact,
                                 link: WidgetLinks.receipts(accountId: entry.accountId))
                }
            }

            Divider()

            if matching.isEmpty {
                Spacer()
                Text(String(localized: "No recent receipts", bundle: .picacho))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                Spacer()
            } else {
                GeometryReader { geo in
                    let fit = max(0, Int((geo.size.height + rowSpacing) / (rowHeight + rowSpacing)))
                    VStack(alignment: .leading, spacing: rowSpacing) {
                        ForEach(Array(matching.prefix(fit))) { rcpt in
                            if compact {
                                receiptRow(rcpt, accent: accent, compact: true, showAccount: false)
                                    .frame(height: rowHeight)
                            } else {
                                Link(destination: WidgetLinks.message(accountId: rcpt.accountId, id: rcpt.mailId)) {
                                    receiptRow(rcpt, accent: accent, compact: false, showAccount: showAccount)
                                        .frame(height: rowHeight)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }
            }
        }
        .widgetURL(WidgetLinks.receipts(accountId: entry.accountId))
    }

    private func receiptRow(_ rcpt: WidgetReceipt, accent: Color, compact: Bool, showAccount: Bool) -> some View {
        let subtitle: String = {
            var parts: [String] = []
            if !compact && !rcpt.order.isEmpty { parts.append(String(localized: "Order \(rcpt.order)", bundle: .picacho, comment: "A receipt's order number")) }
            let d = WidgetFormatting.messageDate(rcpt.updated > 0 ? rcpt.updated : rcpt.date)
            if !d.isEmpty { parts.append(d) }
            if showAccount && !rcpt.accountLabel.isEmpty {
                parts.append(rcpt.accountLabel)
            }
            return parts.joined(separator: " · ")
        }()
        return HStack(alignment: .center, spacing: 6) {
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    Text(rcpt.merchant.isEmpty ? String(localized: "Receipt", bundle: .picacho, comment: "Stands in for a receipt without a shop name") : rcpt.merchant)
                        .privacySensitive()
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    if rcpt.refund && !compact {
                        Text(String(localized: "Refund", bundle: .picacho))
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(accent)
                    }
                }
                if !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            Text(rcpt.amount)
                .privacySensitive()
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(rcpt.refund ? accent : .primary)
                .lineLimit(1)
        }
    }
}

// MARK: - Bundle entry point

@main
struct PicachoWidgetsBundle: WidgetBundle {
    var body: some Widget {
        InboxWidget()
        UnreadWidget()
        CalendarWidget()
        PackagesWidget()
        ReceiptsWidget()
    }
}
