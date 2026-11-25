// The language Picacho shows, for the text its extensions show (widgets,
// the Share sheet): Settings in Picacho may choose one apart from the
// system's. Picacho writes it into the widget snapshot ("language"); an
// extension reads it from there and calls `PicachoLanguage.use`.
import Foundation

enum PicachoLanguage {
    private static let lock = NSLock()
    private static var code = ""

    /// Shows `language` (a BCP 47 tag; empty: the system's) from now on.
    static func use(_ language: String) {
        lock.withLock { code = language }
    }

    /// The strings of the language Picacho shows: its .lproj in this
    /// extension's bundle (pt-BR, else pt), else the system's choice.
    static var bundle: Bundle {
        let language = lock.withLock { code }
        guard !language.isEmpty else { return .main }
        let base = language.split(separator: "-").first.map(String.init) ?? language
        for name in [language, base] {
            if let path = Bundle.main.path(forResource: name, ofType: "lproj"), let found = Bundle(path: path) {
                return found
            }
        }
        return .main
    }

    /// The locale dates are written in.
    static var locale: Locale {
        let language = lock.withLock { code }
        return language.isEmpty ? .current : Locale(identifier: language)
    }
}

extension Bundle {
    /// The strings of the language Picacho shows (`PicachoLanguage`).
    static var picacho: Bundle { PicachoLanguage.bundle }
}
