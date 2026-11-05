struct MixedInputScenario: Sendable {
    let keys: String
    let reading: String
    let literals: [String]
}

enum MixedInputScenarioCorpus {
    // An independent acceptance vocabulary, not read from the implementation.
    static let programmingWords = """
    git github gitlab commit push pull merge branch build test debug bug log input output
    python rust swift javascript typescript kotlin docker kubernetes cluster
    nodejs react flutter npm pnpm yarn pip cargo cmake gradle compiler interpreter namespace
    json yaml html css sql http https ssh tcp udp ssl uuid
    server client backend frontend function class string array object request response
    postgresql mysql sqlite redis mongodb graphql package import export struct enum
    return async await vector regex buffer thread mutex socket packet timeout retry
    checkout fetch clone reset diff grep curl clang llvm sdk cli cpu gpu aws gcp parse parser
    vscode linux windows
    """.split(whereSeparator: \.isWhitespace).map(String.init)

    static let programmingMatrix: [MixedInputScenario] = programmingWords.flatMap { word -> [MixedInputScenario] in
        [
            .init(keys: word + "wotsukau", reading: word + "をつかう", literals: [word]),
            .init(keys: word + "nokakikata", reading: word + "のかきかた", literals: [word]),
            .init(keys: word + "hayoi", reading: word + "はよい", literals: [word]),
            .init(keys: "kyouha" + word + "desu", reading: "きょうは" + word + "です", literals: [word]),
        ]
    }

    static let sentences: [MixedInputScenario] = [
        .init(keys: "gitdebranchwokiru", reading: "gitでbranchをきる", literals: ["git", "branch"]),
        .init(keys: "githubnocommitwomodoshita", reading: "githubのcommitをもどした", literals: ["github", "commit"]),
        .init(keys: "rustdejsonwoparse", reading: "rustでjsonをparse", literals: ["rust", "json", "parse"]),
        .init(keys: "pythonnotestwokaku", reading: "pythonのtestをかく", literals: ["python", "test"]),
        .init(keys: "dockerdenodejswougokasu", reading: "dockerでnodejsをうごかす", literals: ["docker", "nodejs"]),
        .init(keys: "kubernetesdeclusterwokanri", reading: "kubernetesでclusterをかんり", literals: ["kubernetes", "cluster"]),
        .init(keys: "postgresqlnoserverwokidousuru", reading: "postgresqlのserverをきどうする", literals: ["postgresql", "server"]),
        .init(keys: "mysqltorediswotsunagu", reading: "mysqlとredisをつなぐ", literals: ["mysql", "redis"]),
        .init(keys: "httpnoresponsewomiru", reading: "httpのresponseをみる", literals: ["http", "response"]),
        .init(keys: "inputwooutputniwatasu", reading: "inputをoutputにわたす", literals: ["input", "output"]),
        .init(keys: "asyncfunctiondeawaitwotsukau", reading: "asyncfunctionでawaitをつかう", literals: ["asyncfunction", "await"]),
        .init(keys: "threadnomutexwokakuninshita", reading: "threadのmutexをかくにんした", literals: ["thread", "mutex"]),
        .init(keys: "compilerdebuildshita", reading: "compilerでbuildした", literals: ["compiler", "build"]),
        .init(keys: "JSONwoYAMLnihenkansuru", reading: "JSONをYAMLにへんかんする", literals: ["JSON", "YAML"]),
        .init(keys: "TypeScriptnoasyncfunctionwokaku", reading: "TypeScriptのasyncfunctionをかく", literals: ["TypeScript", "asyncfunction"]),
        .init(keys: "GitHubnoHTTPresponsewomiru", reading: "GitHubのHTTPresponseをみる", literals: ["GitHub", "HTTPresponse"]),
        .init(keys: "python3dejsonwoyomu", reading: "python3でjsonをよむ", literals: ["python", "json"]),
        .init(keys: "http2noresponsewoshori", reading: "http2のresponseをしょり", literals: ["http", "response"]),
        .init(keys: "chrometofirefoxwotsukau", reading: "chromeとfirefoxをつかう", literals: ["chrome", "firefox"]),
        .init(keys: "slacktodiscordderenraku", reading: "slackとdiscordでれんらく", literals: ["slack", "discord"]),
        .init(keys: "exceldehyouwohiraku", reading: "excelでひょうをひらく", literals: ["excel"]),
        .init(keys: "powerpointdeshiryouwotsukuru", reading: "powerpointでしりょうをつくる", literals: ["powerpoint"]),
        .init(keys: "keyboardwokaeta", reading: "keyboardをかえた", literals: ["keyboard"]),
        .init(keys: "meetingnojikanwokakuninshita", reading: "meetingのじかんをかくにんした", literals: ["meeting"]),
        .init(keys: "weekendnoyoteiwotateru", reading: "weekendのよていをたてる", literals: ["weekend"]),
        .init(keys: "ticketwokatta", reading: "ticketをかった", literals: ["ticket"]),
        .init(keys: "shoppingnoyoteiwotateru", reading: "shoppingのよていをたてる", literals: ["shopping"]),
        .init(keys: "emaildeokuru", reading: "emailでおくる", literals: ["email"]),
        .init(keys: "musicwokiku", reading: "musicをきく", literals: ["music"]),
        .init(keys: "lunchwotabeta", reading: "lunchをたべた", literals: ["lunch"]),
        .init(keys: "newswoyomu", reading: "newsをよむ", literals: ["news"]),
    ]

    // A corpus of ordinary Japanese and deliberate readable ambiguities. The
    // control is the real romaji table with automatic mixing disabled.
    static let japaneseControls = """
    nihongo konnichiwa ohayou arigatou sayounara yoroshiku onegaishimasu
    watashi anata karera kyou ashita kinou tenki hare ame yuki sora umi yama
    hana kawa neko inu tori sakana gohan sushi tomato tofu ramen ringo banana
    oishii taberu nomu iku kuru kaeru miru kiku yomu kaku hanasu tsukau
    tsukaimasu mottekita motteiku shimasu shimashita dekiru dekita
    gitai kitto shutoken syutoken tokenai apiru maketa sonomama soredeha
    kaisha kaisya shigoto shitsumon kakunin henkan bunshou bunsetsu
    puroguramu puroguramingu hensuu kansuu kurasu mojiretsu nyuuryoku shutsuryoku
    api make go node file issue cache rebase vue java ubuntu
    resetto resettosuru resettoshita resettogadekita resettonoyarinaoshi gitto
    komittoshita debagguwoshusei roguwomiru
    """.split(whereSeparator: \.isWhitespace).map(String.init)

    static let explicitLiterals = [
        "API", "SDK", "CLI", "IDE", "C++", "C#", ".NET", "Node.js", "TypeScript",
        "getUserId", "user_id", "snake_case", "foo.bar", "std::vector", "HTTP/2",
        "UTF-8", "x86_64", "python3.12", "Cargo.toml", "package.json", "npm install",
        "https://example.com/a_b", "C:\\src\\main.rs", "foo@example.com",
        "api", "file", "cache", "rebase", "ubuntu", "vue", "java", "go", "node", "token",
        "coffee", "movie", "youtube", "mouse",
    ]
}
