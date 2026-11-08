import Foundation

public enum Tone: String, Codable, Sendable {
    case info, success, warning, error, primary, danger, `default`

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Tone(rawValue: raw.lowercased()) ?? .default
    }
}

public struct PluginPageRef: Decodable, Identifiable, Hashable, Sendable {
    public let id: String
    public let title: String
    public let icon: String?
    public let nav: Bool?
}

public struct PluginActionRef: Decodable, Identifiable, Hashable, Sendable {
    public let id: String
    public let title: String
    public let icon: String?
    public let contexts: [String]?
}

public struct PluginInfo: Decodable, Identifiable, Hashable, Sendable {
    public let id: String
    public let name: String
    public let version: String?
    public let description: String?
    public let icon: String?
    public let online: Bool?
    public let pages: [PluginPageRef]
    public let actions: [PluginActionRef]
    public let hasSettings: Bool?

    enum CodingKeys: String, CodingKey {
        case id, name, version, description, icon, online, pages, actions
        case hasSettings = "has_settings"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? id
        version = try c.decodeIfPresent(String.self, forKey: .version)
        description = try c.decodeIfPresent(String.self, forKey: .description)
        icon = try c.decodeIfPresent(String.self, forKey: .icon)
        online = try c.decodeIfPresent(Bool.self, forKey: .online)
        pages = try c.decodeIfPresent([PluginPageRef].self, forKey: .pages) ?? []
        actions = try c.decodeIfPresent([PluginActionRef].self, forKey: .actions) ?? []
        hasSettings = try c.decodeIfPresent(Bool.self, forKey: .hasSettings)
    }

    public var navPages: [PluginPageRef] { pages.filter { $0.nav == true } }

    public func actions(for context: String) -> [PluginActionRef] {
        actions.filter { $0.contexts?.contains(context) ?? false }
    }
}

public struct PluginNav: Codable, Hashable, Sendable {
    public let page: String
    public let params: [String: JSONValue]?

    public init(page: String, params: [String: JSONValue]? = nil) {
        self.page = page
        self.params = params
    }

    public var queryItems: [URLQueryItem] {
        (params ?? [:]).sorted { $0.key < $1.key }.compactMap { k, v in v.stringValue.map { URLQueryItem(name: k, value: $0) } }
    }
}

public struct PluginCall: Codable, Hashable, Sendable {
    public let method: String
    public let path: String
    public let body: [String: JSONValue]?

    public init(method: String, path: String, body: [String: JSONValue]? = nil) {
        self.method = method
        self.path = path
        self.body = body
    }
}

public struct PluginUIAction: Decodable, Hashable, Sendable {
    public let label: String
    public let icon: String?
    public let tone: Tone?
    public let confirm: String?
    public let call: PluginCall?
    public let navigate: PluginNav?
}

public struct PluginFieldOption: Decodable, Hashable, Sendable {
    public let value: JSONValue
    public let label: String

    enum CodingKeys: String, CodingKey { case value, label }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        value = try c.decode(JSONValue.self, forKey: .value)
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? value.stringValue ?? ""
    }
}

public struct PluginField: Decodable, Hashable, Sendable, Identifiable {
    public enum Kind: String, Decodable, Sendable {
        case text, password, number, select, toggle, file, textarea

        public init(from decoder: Decoder) throws {
            self = Kind(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .text
        }
    }

    public var id: String { name }
    public let name: String
    public let label: String
    public let kind: Kind
    public let value: JSONValue?
    public let placeholder: String?
    public let help: String?
    public let required: Bool?
    public let options: [PluginFieldOption]?

    enum CodingKeys: String, CodingKey { case name, label, kind, value, placeholder, help, required, options }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? name
        kind = try c.decodeIfPresent(Kind.self, forKey: .kind) ?? .text
        value = try c.decodeIfPresent(JSONValue.self, forKey: .value)
        placeholder = try c.decodeIfPresent(String.self, forKey: .placeholder)
        help = try c.decodeIfPresent(String.self, forKey: .help)
        required = try c.decodeIfPresent(Bool.self, forKey: .required)
        options = try c.decodeIfPresent([PluginFieldOption].self, forKey: .options)
    }

    /// The value a form starts with: the field's own value, or an empty value of the right shape.
    public var initialValue: JSONValue {
        if let value, value != .null { return value }
        switch kind {
        case .toggle: return .bool(false)
        case .select: return options?.first?.value ?? .null
        case .file: return .null
        default: return .string("")
        }
    }
}

public struct PluginStat: Decodable, Hashable, Sendable {
    public let label: String
    public let value: JSONValue
}

public struct PluginBadge: Decodable, Hashable, Sendable {
    public let text: String
    public let tone: Tone?
}

public struct PluginListItem: Decodable, Hashable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let subtitle: String?
    public let image: String?
    public let progress: Double?
    public let badges: [PluginBadge]?
    public let actions: [PluginUIAction]?
    public let link: PluginNav?

    enum CodingKeys: String, CodingKey { case id, title, subtitle, image, progress, badges, actions, link }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        id = (try c.decodeIfPresent(JSONValue.self, forKey: .id))?.stringValue ?? title
        subtitle = try c.decodeIfPresent(String.self, forKey: .subtitle)
        image = try c.decodeIfPresent(String.self, forKey: .image)
        progress = (try c.decodeIfPresent(JSONValue.self, forKey: .progress))?.doubleValue
        badges = try c.decodeIfPresent([PluginBadge].self, forKey: .badges)
        actions = try c.decodeIfPresent([PluginUIAction].self, forKey: .actions)
        link = try c.decodeIfPresent(PluginNav.self, forKey: .link)
    }
}

public struct PluginSubmit: Decodable, Hashable, Sendable {
    public let label: String
    public let call: PluginCall?

    public init(label: String, call: PluginCall? = nil) {
        self.label = label
        self.call = call
    }

    enum CodingKeys: String, CodingKey { case label, call }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? "Save"
        call = try c.decodeIfPresent(PluginCall.self, forKey: .call)
    }
}

public struct PluginForm: Decodable, Hashable, Sendable {
    public let id: String?
    public let fields: [PluginField]
    public let submit: PluginSubmit

    enum CodingKeys: String, CodingKey { case id, fields, submit }

    public init(id: String? = nil, fields: [PluginField], submit: PluginSubmit) {
        self.id = id
        self.fields = fields
        self.submit = submit
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = (try c.decodeIfPresent(JSONValue.self, forKey: .id))?.stringValue
        fields = try c.decodeIfPresent([PluginField].self, forKey: .fields) ?? []
        submit = try c.decodeIfPresent(PluginSubmit.self, forKey: .submit) ?? PluginSubmit(label: "Save")
    }

    /// The request body for a submit: `call.body` merged with the field values (field values win).
    public func body(values: [String: JSONValue]) -> [String: JSONValue] {
        var out = submit.call?.body ?? [:]
        for f in fields {
            let v = values[f.name] ?? f.initialValue
            if f.kind == .number, case .string(let s) = v {
                out[f.name] = Double(s.trimmingCharacters(in: .whitespaces)).map(JSONValue.number) ?? (s.isEmpty ? .null : .string(s))
            } else {
                out[f.name] = v
            }
        }
        return out
    }

    /// Names of required fields that are still empty.
    public func missing(values: [String: JSONValue]) -> [String] {
        fields.filter { $0.required == true }.filter { f in
            switch values[f.name] ?? f.initialValue {
            case .null: return true
            case .string(let s): return s.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            default: return false
            }
        }.map(\.label)
    }

    public static func fileValue(name: String, data: Data) -> JSONValue {
        .object(["name": .string(name), "data_base64": .string(data.base64EncodedString())])
    }
}

public enum PluginBlock: Decodable, Hashable, Sendable {
    case text(String, Tone?)
    case stats([PluginStat])
    case actions([PluginUIAction])
    case form(PluginForm)
    case list(id: String?, items: [PluginListItem], empty: String?)
    case unknown(String)

    enum CodingKeys: String, CodingKey { case type, text, tone, items, id, empty }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "text":
            self = .text(try c.decodeIfPresent(String.self, forKey: .text) ?? "", try c.decodeIfPresent(Tone.self, forKey: .tone))
        case "stats":
            self = .stats(try c.decodeIfPresent([PluginStat].self, forKey: .items) ?? [])
        case "actions":
            self = .actions(try c.decodeIfPresent([PluginUIAction].self, forKey: .items) ?? [])
        case "form":
            self = .form(try PluginForm(from: decoder))
        case "list":
            self = .list(
                id: try c.decodeIfPresent(JSONValue.self, forKey: .id)?.stringValue,
                items: try c.decodeIfPresent([PluginListItem].self, forKey: .items) ?? [],
                empty: try c.decodeIfPresent(String.self, forKey: .empty)
            )
        default:
            self = .unknown(type)
        }
    }
}

public struct PluginView: Decodable, Hashable, Sendable {
    public let title: String?
    public let subtitle: String?
    public let refresh: Double?
    public let blocks: [PluginBlock]

    enum CodingKeys: String, CodingKey { case title, subtitle, refresh, blocks }

    public init(title: String?, subtitle: String? = nil, refresh: Double? = nil, blocks: [PluginBlock]) {
        self.title = title
        self.subtitle = subtitle
        self.refresh = refresh
        self.blocks = blocks
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        subtitle = try c.decodeIfPresent(String.self, forKey: .subtitle)
        refresh = (try c.decodeIfPresent(JSONValue.self, forKey: .refresh))?.doubleValue
        blocks = try c.decodeIfPresent([PluginBlock].self, forKey: .blocks) ?? []
    }
}

public struct PluginResult: Decodable, Hashable, Sendable {
    public let message: String?
    public let tone: Tone?
    public let view: PluginView?
    public let navigate: PluginNav?
    public let refresh: Bool?

    public static let empty = PluginResult(message: nil, tone: nil, view: nil, navigate: nil, refresh: nil)

    public init(message: String?, tone: Tone?, view: PluginView?, navigate: PluginNav?, refresh: Bool?) {
        self.message = message
        self.tone = tone
        self.view = view
        self.navigate = navigate
        self.refresh = refresh
    }
}

public struct MissingEpisode: Hashable, Sendable {
    public let season: Int
    public let episode: Int
    public let title: String?

    public init(season: Int, episode: Int, title: String?) {
        self.season = season
        self.episode = episode
        self.title = title
    }
}

public struct PluginContext: Hashable, Sendable {
    public let kind: String
    public let id: Int
    public let tmdbId: Int?
    public let title: String
    public let year: Int?
    public let season: Int?
    public let missing: [MissingEpisode]?
    public let extra: [String: JSONValue]

    public init(kind: String, id: Int, tmdbId: Int?, title: String, year: Int?, season: Int? = nil, missing: [MissingEpisode]? = nil, extra: [String: JSONValue] = [:]) {
        self.extra = extra
        self.kind = kind
        self.id = id
        self.tmdbId = tmdbId
        self.title = title
        self.year = year
        self.season = season
        self.missing = missing
    }

    public var json: JSONValue {
        var o = extra
        o.merge([
            "kind": .string(kind),
            "id": .number(Double(id)),
            "tmdb_id": tmdbId.map { .number(Double($0)) } ?? .null,
            "title": .string(title),
            "year": year.map { .number(Double($0)) } ?? .null,
        ]) { $1 }
        if let season { o["season"] = .number(Double(season)) }
        if let missing {
            o["missing"] = .array(missing.map {
                .object(["season": .number(Double($0.season)), "episode": .number(Double($0.episode)), "title": $0.title.map(JSONValue.string) ?? .null])
            })
        }
        return .object(o)
    }
}
