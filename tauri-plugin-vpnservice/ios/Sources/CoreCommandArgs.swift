import Foundation

struct CoreCommandArgs: Decodable {
    let command: String
    let url: String?
}
