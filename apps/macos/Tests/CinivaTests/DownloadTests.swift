import CinivaKit
import XCTest

final class DownloadTests: XCTestCase {
    func testDecodesServerDownload() throws {
        let json = """
        [{"available_fraction":0.93,"available_secs":6114.4,"duration":6543.1,"id":"e1af-0","name":"Obsession.2025.mkv",
          "path":"/media/Downloads/Obsession.2025.mkv","playable":true,"progress":0.95,"size":21622600696,"subtitle":null,"title":"Obsession (2025)"},
         {"available_fraction":0,"available_secs":null,"duration":null,"id":"ab-2","name":"Show.S01E02.mkv","path":"/x","playable":false,
          "progress":0.01,"size":1000,"subtitle":"S01E02","title":"Show","probe":null}]
        """
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        let items = try d.decode([DownloadItem].self, from: Data(json.utf8))
        XCTAssertEqual(items.map(\.id), ["e1af-0", "ab-2"])
        XCTAssertTrue(items[0].playable)
        XCTAssertEqual(items[0].availableSecs ?? 0, 6114.4, accuracy: 0.01)
        XCTAssertEqual(items[1].label, "Show · S01E02")
        XCTAssertNil(items[1].availableSecs)
    }
}
