using System.Globalization;
using System.Resources;

namespace Picacho.Widgets;

/// <summary>
/// The widgets' text, from Strings.resx (or its translation,
/// Strings.&lt;language&gt;.resx), in the language Picacho shows: the
/// snapshot names it, and it may not be Windows' display language.
/// </summary>
internal static class L
{
    private static readonly ResourceManager Resources = new("Picacho.Widgets.Strings", typeof(L).Assembly);

    /// <summary>The language the widgets write in (Windows' until a snapshot names Picacho's).</summary>
    public static CultureInfo Culture { get; private set; } = CultureInfo.CurrentUICulture;

    /// <summary>Writes in <paramref name="language"/> (a BCP 47 tag; empty: Windows' language).</summary>
    public static void Use(string language)
    {
        try
        {
            Culture = string.IsNullOrWhiteSpace(language)
                ? CultureInfo.CurrentUICulture
                : CultureInfo.GetCultureInfo(language);
        }
        catch (CultureNotFoundException)
        {
            Culture = CultureInfo.CurrentUICulture;
        }
    }

    /// <summary>The text named <paramref name="key"/>.</summary>
    public static string Get(string key) => Resources.GetString(key, Culture) ?? key;

    /// <summary>The text named <paramref name="key"/>, with {0}, {1}… filled in.</summary>
    public static string Format(string key, params object[] args) =>
        string.Format(Culture, Get(key), args);
}
