using System.Collections.Concurrent;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.Windows.Widgets;
using Microsoft.Windows.Widgets.Providers;

namespace Picacho.Widgets;

/// <summary>
/// Windows 11 Widgets Board provider for Picacho: renders Adaptive Cards for
/// Inbox, Unread Mail, and Calendar from Picacho's widget-snapshot.json and
/// watches for live updates.
/// </summary>
[ComVisible(true)]
[Guid("7F3E2D4A-8C19-4B5E-9A1D-2E6F4C8B3A50")]
public sealed class WidgetProvider : IWidgetProvider
{
    private const string InboxDefinition = "Picacho_Inbox";
    private const string UnreadDefinition = "Picacho_Unread";
    private const string CalendarDefinition = "Picacho_Calendar";
    private const string PackagesDefinition = "Picacho_Packages";
    private const string ReceiptsDefinition = "Picacho_Receipts";

    private static readonly ConcurrentDictionary<string, WidgetInfo> ActiveWidgets = new();
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };
    private static FileSystemWatcher? _watcher;

    public WidgetProvider()
    {
        EnsureWatcher();
        RecoverRunningWidgets();
    }

    public void CreateWidget(WidgetContext widgetContext)
    {
        var info = new WidgetInfo(widgetContext.Id, widgetContext.DefinitionId, widgetContext.Size, "");
        ActiveWidgets[widgetContext.Id] = info;
        UpdateWidget(info);
    }

    public void DeleteWidget(string widgetId, string customState)
    {
        ActiveWidgets.TryRemove(widgetId, out _);
    }

    public void OnActionInvoked(WidgetActionInvokedArgs actionInvokedArgs)
    {
        var id = actionInvokedArgs.WidgetContext.Id;
        var existing = ActiveWidgets.TryGetValue(id, out var found)
            ? found
            : new WidgetInfo(id, actionInvokedArgs.WidgetContext.DefinitionId, actionInvokedArgs.WidgetContext.Size, actionInvokedArgs.CustomState ?? "");

        if (string.Equals(actionInvokedArgs.Verb, "cycleAccount", StringComparison.OrdinalIgnoreCase))
        {
            var snap = LoadSnapshot();
            var nextAccount = NextAccountId(snap, existing.AccountId);
            var updated = existing with { AccountId = nextAccount };
            ActiveWidgets[id] = updated;
            UpdateWidget(updated, snap);
            return;
        }

        if (string.Equals(actionInvokedArgs.Verb, "refresh", StringComparison.OrdinalIgnoreCase))
        {
            ActiveWidgets[id] = existing;
            UpdateWidget(existing);
        }
    }

    public void OnWidgetContextChanged(WidgetContextChangedArgs contextChangedArgs)
    {
        var ctx = contextChangedArgs.WidgetContext;
        var prevAccount = ActiveWidgets.TryGetValue(ctx.Id, out var prev) ? prev.AccountId : "";
        var info = new WidgetInfo(ctx.Id, ctx.DefinitionId, ctx.Size, prevAccount);
        ActiveWidgets[ctx.Id] = info;
        UpdateWidget(info);
    }

    public void Activate(WidgetContext widgetContext)
    {
        var prevAccount = ActiveWidgets.TryGetValue(widgetContext.Id, out var prev) ? prev.AccountId : "";
        var info = new WidgetInfo(widgetContext.Id, widgetContext.DefinitionId, widgetContext.Size, prevAccount);
        ActiveWidgets[widgetContext.Id] = info;
        UpdateWidget(info);
    }

    public void Deactivate(string widgetId)
    {
        // Keep registration so background snapshot updates still reach the widget when reopened.
    }

    private static void RecoverRunningWidgets()
    {
        try
        {
            foreach (var w in WidgetManager.GetDefault().GetWidgetInfos())
            {
                var ctx = w.WidgetContext;
                ActiveWidgets[ctx.Id] = new WidgetInfo(ctx.Id, ctx.DefinitionId, ctx.Size, w.CustomState ?? "");
            }
        }
        catch
        {
            // Widgets host may not be ready yet during early initialization.
        }
    }

    private static string NextAccountId(WidgetSnapshot snap, string currentAccountId)
    {
        if (snap.Accounts.Count <= 1) return "";
        if (string.IsNullOrEmpty(currentAccountId)) return snap.Accounts[0].Id;
        var idx = snap.Accounts.FindIndex(a => a.Id == currentAccountId);
        return (idx >= 0 && idx + 1 < snap.Accounts.Count) ? snap.Accounts[idx + 1].Id : "";
    }

    private static string AccountLabelFor(WidgetSnapshot snap, string accountId)
    {
        if (!string.IsNullOrEmpty(accountId))
        {
            var found = snap.Accounts.FirstOrDefault(a => a.Id == accountId);
            if (found != null) return string.IsNullOrWhiteSpace(found.Name) ? found.Email : found.Name;
        }
        if (snap.Accounts.Count == 1)
        {
            var only = snap.Accounts[0];
            return string.IsNullOrWhiteSpace(only.Name) ? only.Email : only.Name;
        }
        return "All Accounts";
    }

    private static void EnsureWatcher()
    {
        if (_watcher != null) return;
        var dir = SnapshotDirectory();
        try
        {
            Directory.CreateDirectory(dir);
            var watcher = new FileSystemWatcher(dir, "widget-snapshot.json")
            {
                NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.CreationTime,
                EnableRaisingEvents = true
            };
            FileSystemEventHandler handler = (_, _) => UpdateAllWidgets();
            RenamedEventHandler renamed = (_, _) => UpdateAllWidgets();
            watcher.Changed += handler;
            watcher.Created += handler;
            watcher.Renamed += renamed;
            _watcher = watcher;
        }
        catch
        {
            // Explicit refreshes and Activate still read the snapshot directly.
        }
    }

    private static void UpdateAllWidgets()
    {
        var snap = LoadSnapshot();
        foreach (var info in ActiveWidgets.Values)
        {
            UpdateWidget(info, snap);
        }
    }

    private static void UpdateWidget(WidgetInfo info, WidgetSnapshot? preloaded = null)
    {
        var snap = preloaded ?? LoadSnapshot();
        var (templateFile, dataObj) = info.DefinitionId switch
        {
            UnreadDefinition => ("unread.json", BuildUnreadData(snap)),
            CalendarDefinition => ("calendar.json", BuildCalendarData(snap, info.Size)),
            PackagesDefinition => ("packages.json", BuildPackagesData(snap, info.Size, info.AccountId)),
            ReceiptsDefinition => ("receipts.json", BuildReceiptsData(snap, info.Size, info.AccountId)),
            _ => ("inbox.json", BuildInboxData(snap, info.Size))
        };

        var templatePath = Path.Combine(AppContext.BaseDirectory, "Templates", templateFile);
        if (!File.Exists(templatePath)) return;

        var options = new WidgetUpdateRequestOptions(info.Id)
        {
            Template = File.ReadAllText(templatePath),
            Data = JsonSerializer.Serialize(dataObj),
            CustomState = info.AccountId ?? ""
        };
        try
        {
            WidgetManager.GetDefault().UpdateWidget(options);
        }
        catch
        {
            // Ignore transient host disconnects.
        }
    }

    private static object BuildInboxData(WidgetSnapshot snap, WidgetSize size)
    {
        int limit = RowsFor(size);
        var label = snap.Accounts.Count == 1 ? snap.Accounts[0].Name : "All Inboxes";
        var rows = snap.Messages.Take(limit).Select(m => new
        {
            sender = string.IsNullOrWhiteSpace(m.Sender) ? "(Unknown sender)" : m.Sender,
            subject = string.IsNullOrWhiteSpace(m.Subject) ? "(No subject)" : m.Subject,
            snippet = size == WidgetSize.Small ? "" : (m.Snippet ?? ""),
            date = FormatShortDate(m.Date),
            unread = m.Unread,
            url = $"picacho://widget/message?account={Uri.EscapeDataString(m.AccountId)}&id={Uri.EscapeDataString(m.Id)}"
        }).ToList();

        return new
        {
            title = string.IsNullOrWhiteSpace(label) ? "Inbox" : label,
            unreadCount = snap.TotalUnread,
            badgeText = snap.TotalUnread > 999 ? "999+" : snap.TotalUnread.ToString(CultureInfo.InvariantCulture),
            inboxUrl = "picacho://widget/inbox",
            composeUrl = "picacho://widget/compose",
            hasMessages = rows.Count > 0,
            emptyText = snap.Accounts.Count == 0 ? "Open Picacho to add an account" : "No messages in Inbox",
            messages = rows
        };
    }

    private static object BuildUnreadData(WidgetSnapshot snap)
    {
        var label = snap.Accounts.Count == 1 ? snap.Accounts[0].Name : "All Inboxes";
        var latest = snap.Messages.FirstOrDefault(m => m.Unread);
        return new
        {
            accountLabel = string.IsNullOrWhiteSpace(label) ? "Inbox" : label,
            unreadCount = snap.TotalUnread,
            unreadText = snap.TotalUnread > 999 ? "999+" : snap.TotalUnread.ToString(CultureInfo.InvariantCulture),
            subtitle = snap.TotalUnread == 0 ? "All caught up" : (snap.TotalUnread == 1 ? "unread message" : "unread messages"),
            inboxUrl = "picacho://widget/inbox",
            composeUrl = "picacho://widget/compose",
            hasLatest = latest != null,
            latestSender = latest?.Sender ?? "",
            latestSubject = string.IsNullOrWhiteSpace(latest?.Subject) ? "(No subject)" : latest!.Subject,
            latestUrl = latest == null
                ? "picacho://widget/inbox"
                : $"picacho://widget/message?account={Uri.EscapeDataString(latest.AccountId)}&id={Uri.EscapeDataString(latest.Id)}"
        };
    }

    private static object BuildCalendarData(WidgetSnapshot snap, WidgetSize size)
    {
        int limit = RowsFor(size);
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var events = snap.Events
            .Where(e => e.AllDay || Math.Max(e.End, e.Start + 60) > now)
            .Take(limit)
            .Select(e => new
            {
                title = string.IsNullOrWhiteSpace(e.Title) ? "(No title)" : e.Title,
                time = FormatEventTime(e),
                location = size == WidgetSize.Small ? "" : (e.Location ?? "")
            })
            .ToList();

        return new
        {
            todayLabel = DateTime.Now.ToString("ddd, MMM d", CultureInfo.CurrentCulture),
            hasEvents = events.Count > 0,
            emptyText = snap.CalendarConnected ? "No upcoming events" : "Connect a calendar in Settings",
            events
        };
    }

    /// <summary>
    /// Rows a Packages or Receipts card shows at each Widgets Board size: as
    /// many two-line rows as its fixed height fits (the board doesn't scroll
    /// or scale cards); "View all N" in the header opens the rest.
    /// </summary>
    private static int RowsFor(WidgetSize size) => size switch
    {
        WidgetSize.Small => 2,
        WidgetSize.Large => 9,
        _ => 5
    };

    /// <summary>"View all N": how many the Summary lists for the account (or all).</summary>
    private static int SummaryTotal(WidgetSnapshot snap, string accountId, bool packages, int held)
    {
        bool filtered = !string.IsNullOrEmpty(accountId);
        int counted = snap.Accounts
            .Where(a => !filtered || a.Id == accountId)
            .Sum(a => packages ? a.Packages : a.Receipts);
        return Math.Max(counted, held);
    }

    private static object BuildPackagesData(WidgetSnapshot snap, WidgetSize size, string accountId)
    {
        int limit = size switch
        {
            WidgetSize.Small => 2,
            WidgetSize.Large => 6,
            _ => 3
        };
        bool filtered = !string.IsNullOrEmpty(accountId);
        bool showAccount = !filtered && snap.Accounts.Count > 1;
        var matching = snap.Packages
            .Where(p => !filtered || p.AccountId == accountId)
            .ToList();
        var packagesUrl = filtered
            ? $"picacho://widget/packages?account={Uri.EscapeDataString(accountId)}"
            : "picacho://widget/packages";
        var rows = matching.Take(limit).Select(p =>
        {
            var parts = new List<string>();
            if (!string.IsNullOrWhiteSpace(p.Status)) parts.Add(p.Status);
            if (!string.IsNullOrWhiteSpace(p.Carrier)) parts.Add(p.Carrier);
            if (!string.IsNullOrWhiteSpace(p.Expected)) parts.Add($"Expected {p.Expected}");
            else
            {
                var d = FormatShortDate(p.Updated);
                if (!string.IsNullOrEmpty(d)) parts.Add(d);
            }
            if (showAccount && !string.IsNullOrWhiteSpace(p.AccountLabel)) parts.Add(p.AccountLabel);
            var url = (!string.IsNullOrEmpty(p.AccountId) && !string.IsNullOrEmpty(p.MailId))
                ? $"picacho://widget/message?account={Uri.EscapeDataString(p.AccountId)}&id={Uri.EscapeDataString(p.MailId)}"
                : packagesUrl;
            return new
            {
                title = string.IsNullOrWhiteSpace(p.Title) ? "Package" : p.Title,
                detail = string.Join(" · ", parts),
                url
            };
        }).ToList();

        return new
        {
            accountLabel = AccountLabelFor(snap, accountId),
            hasMultipleAccounts = snap.Accounts.Count > 1,
            packagesUrl,
            hasPackages = rows.Count > 0,
            viewAllText = "View all " + SummaryTotal(snap, accountId, true, matching.Count)
                .ToString(CultureInfo.InvariantCulture),
            packages = rows
        };
    }

    private static object BuildReceiptsData(WidgetSnapshot snap, WidgetSize size, string accountId)
    {
        int limit = size switch
        {
            WidgetSize.Small => 2,
            WidgetSize.Large => 6,
            _ => 3
        };
        bool filtered = !string.IsNullOrEmpty(accountId);
        bool showAccount = !filtered && snap.Accounts.Count > 1;
        var matching = snap.Receipts
            .Where(r => !filtered || r.AccountId == accountId)
            .ToList();
        var receiptsUrl = filtered
            ? $"picacho://widget/receipts?account={Uri.EscapeDataString(accountId)}"
            : "picacho://widget/receipts";
        var rows = matching.Take(limit).Select(r =>
        {
            var parts = new List<string>();
            if (!string.IsNullOrWhiteSpace(r.Order)) parts.Add($"Order {r.Order}");
            var d = FormatShortDate(r.Updated > 0 ? r.Updated : r.Date);
            if (!string.IsNullOrEmpty(d)) parts.Add(d);
            if (showAccount && !string.IsNullOrWhiteSpace(r.AccountLabel)) parts.Add(r.AccountLabel);
            var url = (!string.IsNullOrEmpty(r.AccountId) && !string.IsNullOrEmpty(r.MailId))
                ? $"picacho://widget/message?account={Uri.EscapeDataString(r.AccountId)}&id={Uri.EscapeDataString(r.MailId)}"
                : receiptsUrl;
            return new
            {
                merchant = string.IsNullOrWhiteSpace(r.Merchant) ? "Receipt" : r.Merchant,
                detail = string.Join(" · ", parts),
                amount = string.IsNullOrWhiteSpace(r.Amount) ? "—" : r.Amount,
                refund = r.Refund,
                url
            };
        }).ToList();

        return new
        {
            accountLabel = AccountLabelFor(snap, accountId),
            hasMultipleAccounts = snap.Accounts.Count > 1,
            receiptsUrl,
            hasReceipts = rows.Count > 0,
            viewAllText = "View all " + SummaryTotal(snap, accountId, false, matching.Count)
                .ToString(CultureInfo.InvariantCulture),
            receipts = rows
        };
    }

    private static string SnapshotDirectory()
    {
        var custom = Environment.GetEnvironmentVariable("PICACHO_DATA_DIR");
        if (!string.IsNullOrWhiteSpace(custom)) return custom;
        // Matches directories::ProjectDirs::from("tech", "pinal", "picacho").data_dir() on Windows.
        var roaming = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        return Path.Combine(roaming, "pinal", "picacho", "data");
    }

    private static WidgetSnapshot LoadSnapshot()
    {
        var path = Path.Combine(SnapshotDirectory(), "widget-snapshot.json");
        try
        {
            if (File.Exists(path))
            {
                var json = File.ReadAllText(path);
                return JsonSerializer.Deserialize<WidgetSnapshot>(json, JsonOptions) ?? new WidgetSnapshot();
            }
        }
        catch
        {
            // Fall back to empty snapshot while a concurrent atomic rename finishes.
        }
        return new WidgetSnapshot();
    }

    private static string FormatShortDate(long epochSeconds)
    {
        if (epochSeconds <= 0) return "";
        var local = DateTimeOffset.FromUnixTimeSeconds(epochSeconds).LocalDateTime;
        var now = DateTime.Now;
        if (local.Date == now.Date) return local.ToString("t", CultureInfo.CurrentCulture);
        if ((now.Date - local.Date).TotalDays is >= 0 and < 6) return local.ToString("ddd", CultureInfo.CurrentCulture);
        return local.Year == now.Year
            ? local.ToString("MMM d", CultureInfo.CurrentCulture)
            : local.ToString("MMM d, yyyy", CultureInfo.CurrentCulture);
    }

    private static string FormatEventTime(WidgetEvent ev)
    {
        var today = DateTime.Today;
        var tomorrow = today.AddDays(1);
        if (ev.AllDay)
        {
            var utc = DateTimeOffset.FromUnixTimeSeconds(ev.Start).UtcDateTime.Date;
            return $"{DayLabel(today, tomorrow, utc)} · All day";
        }
        var start = DateTimeOffset.FromUnixTimeSeconds(ev.Start).LocalDateTime;
        var day = DayLabel(today, tomorrow, start.Date);
        var startStr = start.ToString("t", CultureInfo.CurrentCulture);
        if (ev.End > ev.Start)
        {
            var end = DateTimeOffset.FromUnixTimeSeconds(ev.End).LocalDateTime;
            return $"{day} · {startStr} – {end.ToString("t", CultureInfo.CurrentCulture)}";
        }
        return $"{day} · {startStr}";
    }

    private static string DayLabel(DateTime today, DateTime tomorrow, DateTime date)
    {
        if (date == today) return "Today";
        if (date == tomorrow) return "Tomorrow";
        return date.ToString("ddd, MMM d", CultureInfo.CurrentCulture);
    }

    private sealed record WidgetInfo(string Id, string DefinitionId, WidgetSize Size, string AccountId);

    private sealed class WidgetSnapshot
    {
        [JsonPropertyName("total_unread")] public int TotalUnread { get; set; }
        [JsonPropertyName("calendar_connected")] public bool CalendarConnected { get; set; }
        [JsonPropertyName("accounts")] public List<AccountSummary> Accounts { get; set; } = new();
        [JsonPropertyName("messages")] public List<WidgetMessage> Messages { get; set; } = new();
        [JsonPropertyName("events")] public List<WidgetEvent> Events { get; set; } = new();
        [JsonPropertyName("packages")] public List<WidgetPackage> Packages { get; set; } = new();
        [JsonPropertyName("receipts")] public List<WidgetReceipt> Receipts { get; set; } = new();
    }

    private sealed class AccountSummary
    {
        [JsonPropertyName("id")] public string Id { get; set; } = "";
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("email")] public string Email { get; set; } = "";
        [JsonPropertyName("unread")] public int Unread { get; set; }
        /// <summary>Packages and receipts the Summary lists for this account.</summary>
        [JsonPropertyName("packages")] public int Packages { get; set; }
        [JsonPropertyName("receipts")] public int Receipts { get; set; }
    }

    private sealed class WidgetMessage
    {
        [JsonPropertyName("id")] public string Id { get; set; } = "";
        [JsonPropertyName("account_id")] public string AccountId { get; set; } = "";
        [JsonPropertyName("sender")] public string Sender { get; set; } = "";
        [JsonPropertyName("subject")] public string Subject { get; set; } = "";
        [JsonPropertyName("snippet")] public string Snippet { get; set; } = "";
        [JsonPropertyName("date")] public long Date { get; set; }
        [JsonPropertyName("unread")] public bool Unread { get; set; }
    }

    private sealed class WidgetEvent
    {
        [JsonPropertyName("title")] public string Title { get; set; } = "";
        [JsonPropertyName("location")] public string Location { get; set; } = "";
        [JsonPropertyName("start")] public long Start { get; set; }
        [JsonPropertyName("end")] public long End { get; set; }
        [JsonPropertyName("all_day")] public bool AllDay { get; set; }
    }

    private sealed class WidgetPackage
    {
        [JsonPropertyName("title")] public string Title { get; set; } = "";
        [JsonPropertyName("carrier")] public string Carrier { get; set; } = "";
        [JsonPropertyName("status")] public string Status { get; set; } = "";
        [JsonPropertyName("expected")] public string Expected { get; set; } = "";
        [JsonPropertyName("updated")] public long Updated { get; set; }
        [JsonPropertyName("account_id")] public string AccountId { get; set; } = "";
        [JsonPropertyName("account_label")] public string AccountLabel { get; set; } = "";
        [JsonPropertyName("mail_id")] public string MailId { get; set; } = "";
    }

    private sealed class WidgetReceipt
    {
        [JsonPropertyName("merchant")] public string Merchant { get; set; } = "";
        [JsonPropertyName("amount")] public string Amount { get; set; } = "";
        [JsonPropertyName("refund")] public bool Refund { get; set; }
        [JsonPropertyName("order")] public string Order { get; set; } = "";
        [JsonPropertyName("date")] public long Date { get; set; }
        [JsonPropertyName("updated")] public long Updated { get; set; }
        [JsonPropertyName("account_id")] public string AccountId { get; set; } = "";
        [JsonPropertyName("account_label")] public string AccountLabel { get; set; } = "";
        [JsonPropertyName("mail_id")] public string MailId { get; set; } = "";
    }
}
