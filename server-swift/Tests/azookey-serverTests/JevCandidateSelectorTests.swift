import Foundation
import Testing
@testable import azookey_server

@Test func jevFFIRequestDecodesCredentialWithoutNetwork() throws {
    let payload = #"{"apiKey":"fixture-key","reading":"きしゃ","context":"列車","candidates":["記者","汽車"]}"#
    let request = try JSONDecoder().decode(JevCandidateFFIRequest.self, from: Data(payload.utf8))
    #expect(request.apiKey == "fixture-key")
    #expect(request.candidates == ["記者", "汽車"])
}

private func selectorResponse(
    choice: String = "c0",
    probabilities: [String: Double] = ["c0": 0.8, "c1": 0.2]
) -> Data {
    Data("""
    {"answers":{"g0":{"type":"choice","choice":"\(choice)","probabilities":\(jsonObject(probabilities))}}}
    """.utf8)
}

private func jsonObject(_ values: [String: Double]) -> String {
    let data = try! JSONSerialization.data(withJSONObject: values, options: [.sortedKeys])
    return String(decoding: data, as: UTF8.self)
}

@Test func jevCandidateSelectorMapsOneCompleteCandidateChoice() throws {
    var capturedBody: Data?
    var capturedTimeout: TimeInterval?
    let selector = JevCandidateSelector(apiKey: "test-key", transport: { body, timeout in
        capturedBody = body
        capturedTimeout = timeout
        return selectorResponse(choice: "c1", probabilities: ["c0": 0.2, "c1": 0.8])
    })

    let selected = try selector.select(
        reading: "きしゃ",
        context: "新聞について",
        candidates: ["記者", "汽車"]
    )

    #expect(selected == 1)
    #expect(capturedTimeout == JevCandidateSelector.defaultTimeout)
    let body = try #require(capturedBody)
    let decoded = try JSONSerialization.jsonObject(with: body)
    let object = try #require(decoded as? [String: Any])
    #expect(object["model"] as? String == "jev-latest")
    let questions = try #require(object["questions"] as? [String: Any])
    let group = try #require(questions["g0"] as? [String: Any])
    #expect(group["type"] as? String == "choice")
    let instructions = try #require(group["instructions"] as? [String: Any])
    #expect(instructions["reading"] as? String == "きしゃ")
    #expect(instructions["context"] as? String == "新聞について")
    let criteria = try #require(group["criteria"] as? [String: Any])
    #expect((criteria["c0"] as? [String: Any])?["text"] as? String == "記者")
    #expect((criteria["c1"] as? [String: Any])?["text"] as? String == "汽車")
}

@Test func jevCandidateSelectorUsesDeterministicProbabilityArgmax() throws {
    let selector = JevCandidateSelector(apiKey: "test-key", transport: { _, _ in
        selectorResponse(choice: "c1", probabilities: ["c0": 0.5, "c1": 0.5])
    })

    let selected = try selector.select(reading: "きしゃ", context: "", candidates: ["記者", "汽車"])

    #expect(selected == 0)
}

@Test func jevCandidateSelectorRejectsMalformedProbabilities() {
    let malformed = [
        #"{"answers":{"g0":{"type":"choice","choice":"c0","probabilities":{"c0":0.5}}}}"#,
        #"{"answers":{"g0":{"type":"choice","choice":"c0","probabilities":{"c0":0.5,"c1":-0.5}}}}"#,
        #"{"answers":{"g0":{"type":"choice","choice":"c0","probabilities":{"c0":0,"c1":0}}}}"#,
        #"{"answers":{"g0":{"type":"choice","choice":"c2","probabilities":{"c0":0.5,"c1":0.5}}}}"#,
    ]

    for response in malformed {
        let selector = JevCandidateSelector(apiKey: "test-key", transport: { _, _ in Data(response.utf8) })
        #expect(throws: JevCandidateSelectorError.self) {
            try selector.select(reading: "きしゃ", context: "", candidates: ["記者", "汽車"])
        }
    }
}

@Test func jevCandidateSelectorRejectsTransportAndInputErrors() {
    let transportFailure = JevCandidateSelector(apiKey: "test-key", transport: { _, _ in
        throw URLError(.timedOut)
    })
    #expect(throws: JevCandidateSelectorError.self) {
        try transportFailure.select(reading: "きしゃ", context: "", candidates: ["記者"])
    }

    let invalidInput = JevCandidateSelector(apiKey: "test-key", transport: { _, _ in
        selectorResponse()
    })
    #expect(throws: JevCandidateSelectorError.self) {
        try invalidInput.select(reading: "", context: "", candidates: ["記者"])
    }
    #expect(throws: JevCandidateSelectorError.self) {
        try invalidInput.select(
            reading: "きしゃ",
            context: "",
            candidates: Array(repeating: "候補", count: JevCandidateSelector.maximumCandidates + 1)
        )
    }
}
