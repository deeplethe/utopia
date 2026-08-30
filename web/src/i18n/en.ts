// 英文语言包。这一份是**结构的权威**：`Strings = typeof en`，其余语言包按它定型，
// 漏一条就编译不过（见 docs/decisions/0004）。
//
// 加新文案时先加在这里，再补其余语言包——顺序反了会得到一个类型错误，那正是本意。
export const en = {
  app: {
    name: "Utopia",
    // 化用《乌托邦》全书最后一句（Burnet 1684 译本）：
    // "there are many things in the commonwealth of Utopia that I rather wish,
    //  than hope, to see followed in our governments."
    // 改 I 为 We、去掉插入语逗号、留白 to see 的宾语。
    tagline: "We rather wish than hope to see.",
    taglineSource: "— Thomas More, 1516",
    siteUrl: "https://utopia.bi",
    docsUrl: "https://utopia.bi/docs",
  },
  /** 服务端校验错误的措辞。key = 服务端给的 code；缺一条就退回英文原句，不会崩。
      契约守卫（调错接口才碰得到）刻意不在这里——它们的读者是开发者 */
  err: {
    bad_email: "That doesn't look like an email address.",
    password_too_short: "Password must be at least 8 characters.",
    bad_display_name: "Display name must be 1-64 characters.",
    wrong_password: "Current password is incorrect.",
    registration_closed:
      "Sign-up is closed on this deployment — ask an administrator for an account.",
    no_chat_model:
      "No chat model configured yet. Set one under Settings → Models.",
    bad_upload: "That upload could not be read.",
    upload_read_failed: "The file could not be read to the end.",
    no_files: "No file was attached.",
    upload_needs_folder: "Files can only be uploaded into a folder source.",
    empty_file: "That file is empty.",
    file_too_large: "That file is too large — the limit is 8 MB.",
    bad_ontology_file: "That is not a readable OWL or RDFS file.",
    bad_name: "Name must be 1-64 characters.",
    default_kb_open:
      "The default knowledge base stays open to everyone — create a separate one for private work.",
    default_kb_undeletable: "The default knowledge base cannot be deleted.",
    last_owner_demote: "This is the last owner — promote someone else first.",
    last_owner_remove: "This is the last owner — hand ownership over first.",
    key_required: "A key is required.",
    forms_required: "Pick at least one phrase this relation covers.",
    bad_key:
      "Keys are lowercase, letters digits and underscores only, up to 40 characters.",
    self_parent: "A class cannot be its own parent.",
    parent_cycle:
      "That class is already below this one — the hierarchy would loop.",
    bad_lang: "Pick a supported language.",
    attr_needs_class: "An attribute has to belong to a class.",
    entity_name_required: "Name cannot be empty.",
    entity_name_too_long: "Name is too long — 100 characters at most.",
    unknown_entity_type: "That class is not in this ontology.",
    nothing_to_update: "Nothing to change.",
    self_merge: "An entity cannot be merged into itself.",
    close_at_required:
      "Pick the date this fact ended — the new one does not say when it started.",
    empty_query: "Type something to search for.",
    no_data_sources: "No databases are mounted on this knowledge base.",
    memory_source_permanent:
      "The Memory source is part of the knowledge base and stays.",
    source_name_required: "Give this source a name.",
    bad_cron: "That cron expression could not be parsed.",
    bad_cron_fields:
      "A cron expression has five fields: minute hour day month weekday.",
    ds_name_required: "Give this data source a name.",
    only_postgres: "Only PostgreSQL is supported for now.",
    bad_conn_string: "A connection string starts with postgres://",
    concurrency_range: "Pick a number between 1 and 256.",
  },
  /** 机器给的补充（cron 解析器的原话之类）缀在措辞后面 */
  errDetail: (msg: string, detail: string) => `${msg} (${detail})`,
  toast: {
    saved: "Saved",
    created: "Created",
    deleted: "Deleted",
    added: "Added to the ontology",
  },
  account: {
    /* 账户区字标：Persona——你在这座城里的身份面具 */
    brand: "Utopia Persona",
    /* 网页标题用的短名：`Utopia | Persona` */
    titleTag: "Persona",
    profile: "Profile",
    administration: "Administration",
    adminChip: "Admin",
    backToApp: "← Back to app",
    profileTitle: "Profile",
    displayName: "Display name",
    email: "Email",
    save: "Save",
    passwordTitle: "Change password",
    currentPassword: "Current password",
    newPassword: "New password (min. 8 characters)",
    changePassword: "Update password",
    passwordChanged: "Password updated",
    avatarHint: "Avatars are generated from your name for now.",
    language: "Language",
    kbsNav: "Knowledge bases",
    kbsTitle: "Knowledge bases",
    kbOpen: "Open",
    kbRestricted: "Restricted",
    kbStats: (docs: number, members: number) =>
      `${docs} doc${docs === 1 ? "" : "s"} · ${members} member${members === 1 ? "" : "s"}`,
    addedBy: (name: string, date: string) => `Added by ${name} · ${date}`,
    joinedOn: (date: string) => `Joined ${date}`,
    openToEveryone: "Open to everyone",
    deploymentAdmin: "Deployment admin",
    openKb: "Open",
    kbSettingsBtn: "Settings",
    roleNames: {
      owner: "Owner",
      admin: "Admin",
      editor: "Editor",
      viewer: "Viewer",
    } as Record<string, string>,
  },
  docs: {
    /* 文档区字标：Charter——理想之城的立城宪章，与主字标同字体同字号 */
    brand: "Utopia Charter",
    backTitle: "Back to Utopia",
    searchPlaceholder: "Search the docs…",
    noResults: "No matches.",
  },
  // 告警的措辞在客户端，按 kind 查——服务端只发 kind 与 detail，
  // 不产出展示文案（docs/decisions/0004）
  alerts: {
    title: "Alerts",
    badgeLabel: "Alerts",
    empty: "Nothing needs attention",
    emptyHint:
      "Ingestion, sync and model failures show up here instead of only in the logs.",
    markAllRead: "Mark all read",
    close: "Close",
    // 说清搜的是什么：标题的措辞在客户端，服务端搜不到它，
    // 所以别让人以为输入 "sync failed" 会有结果
    searchPlaceholder: "Search sources, knowledge bases, errors",
    noMatch: "Nothing matches",
    andMore: (n: number) => `and ${n} more`,
    system: "System",
    // kind → 一句说清出了什么事。第二句说该做什么——这才是告警比日志多出来的东西。
    // **一条告警就是一次故障**，所以标题里没有数量
    kinds: {
      "source.sync_failed": {
        title: "A source failed to sync",
        hint: "Nothing new came in from it. Check the source's settings.",
      },
      "llm.unreachable": {
        title: "The model endpoint gave no usable answer",
        hint: "Extraction and embedding are stopped. Check the endpoint URL in system settings.",
      },
    } as Record<string, { title: string; hint: string } | undefined>,
    // 没见过的 kind 也要能显示：新告警源上线时前端可能还没更新
    unknownKind: (kind: string) => kind,
  },

  nav: {
    workspaceLabel: "Workspace",
    kbLabel: "Knowledge base",
    ask: "Chat",
    askHint: "Converse with your knowledge base — it can remember",
    search: "Search",
    searchHint: "Hybrid search",
    graph: "Graph",
    graphHint: "Entities & timelines",
    library: "Library",
    libraryHint: "Documents & ingestion",
    settings: "Settings",
    settingsHint: "Models & members",
    signOut: "Sign out",
    docs: "Docs",
    loading: "Loading…",
    serverUnreachable:
      "Punishment 500: Utopia has gone quiet — it isn't answering.",
    notFound: "Punishment 404: You are lost in Utopia.",
    returnHome: "Return home",
    reportIssue: "Report an issue",
    refresh: "Refresh",
  },
  login: {
    signIn: "Sign in",
    signUp: "Sign up",
    displayName: "Display name",
    email: "Email",
    password: "Password (min. 8 characters)",
    submitting: "One moment…",
    createAccount: "Create account",
    networkError: "Network error, please try again",
    // 惯用同意句式：By continuing, you agree to the <Terms> and acknowledge the <Privacy>.
    agreePrefix: "By continuing, you agree to the ",
    agreeAnd: " and acknowledge the ",
    agreeSuffix: ".",
    githubUrl: "https://github.com/deeplethe/utopia",
  },
  legal: {
    privacyTitle: "Privacy policy",
    termsTitle: "Terms of use",
    backToSignIn: "← Back to sign in",
    privacy: {
      title: "Privacy policy",
      note: "Default text bundled with Utopia. The organization operating this deployment may replace it with its own policy.",
      sections: [
        {
          h: "A self-hosted platform",
          body: [
            "Utopia runs entirely on infrastructure chosen by the organization that deployed it (the operator). The Utopia project has no access to this deployment: the software sends no telemetry, no analytics and no crash reports to anyone.",
          ],
        },
        {
          h: "What this instance stores",
          body: ["Everything below lives on the operator's own servers:"],
          bullets: [
            "Account details — your email address, display name and a hash of your password.",
            "Content — uploaded documents, text extracted from them, search indexes and embeddings, and the knowledge graph (entities, relations and their sources) built from that content.",
            "Activity — your conversations with the assistant, review decisions and ingestion logs, kept so the features that need them can work.",
          ],
        },
        {
          h: "Where data can leave this server",
          body: [
            "If the operator configures an external model provider for chat or embeddings, excerpts of documents and your messages are sent to that provider to answer questions and index content. Which provider — or whether a fully local model is used — is a deployment setting. Nothing else is sent anywhere.",
          ],
        },
        {
          h: "Who can see what",
          body: [
            "Access follows knowledge-base roles: viewers see the knowledge bases they were granted, editors can change content, admins manage members and settings. Deployment administrators can create accounts and see the user list.",
          ],
        },
        {
          h: "Retention and deletion",
          body: [
            "Deleting a document removes its stored content and index entries. Facts already extracted into the knowledge graph remain, with their provenance, until removed through Review. Deleting a knowledge base permanently removes its documents, graph and sources.",
          ],
        },
        {
          h: "Questions",
          body: [
            "This deployment is run by your organization. For questions about how your data is handled here, contact its administrator.",
          ],
        },
      ],
    },
    terms: {
      title: "Terms of use",
      note: "Default text bundled with Utopia. The organization operating this deployment may replace it with its own terms.",
      sections: [
        {
          h: "About these terms",
          body: [
            "This instance of Utopia is operated by the organization that deployed it, not by the Utopia project. Your use of it is governed by that organization's own policies; these default terms cover the basics until the operator replaces them.",
          ],
        },
        {
          h: "Your account",
          body: [
            "Keep your credentials to yourself. Administrators may create, suspend or remove accounts in line with the operator's policies.",
          ],
        },
        {
          h: "Acceptable use",
          body: [],
          bullets: [
            "Upload only content you are authorized to store and share within your organization.",
            "Respect access levels: do not attempt to view or change knowledge bases beyond the roles you were granted.",
            "Do not use the platform to store or spread unlawful content.",
          ],
        },
        {
          h: "AI-generated answers",
          body: [
            "Answers from the assistant are generated from your organization's documents by a language model, with citations. They can be wrong or incomplete — verify against the cited sources before relying on them.",
          ],
        },
        {
          h: "The software",
          body: [
            "Utopia is open-source software provided “as is”, without warranty of any kind. Responsibility for operating this deployment — including backups, availability and compliance — lies with the operator.",
          ],
        },
      ],
    },
  },
  library: {
    title: "Library",
    upload: "Upload files",
    uploading: "Uploading…",
    uploadFailed: "Upload failed",
    dropHint: "Drag files here, or click “Upload files”",
    formats:
      "PDF · Word · Excel · PowerPoint · Markdown · HTML · TXT · CSV and more",
    emptyPull: "No documents yet — they arrive when this source syncs.",
    filterPlaceholder: "Filter by name",
    filterNoMatch: "No documents match your filter.",
    colFile: "File",
    colStatus: "Status",
    colGraph: "Graph",
    colChunks: "Chunks",
    colSize: "Size",
    colSource: "Source",
    delete: "Delete",
    extract: "Extract",
    reExtract: "Re-extract",
    reprocess: "Reprocess",
    // 抽取进度（当前视图内聚合，SSE 推动刷新）
    extractProgress: (done: number, total: number) =>
      `Extracting · ${done} / ${total}`,
    // 失败详情：chip 可点开，不再只有 tooltip
    errorTitle: "Failure details",
    errorParse: "Ingestion pipeline",
    errorGraph: "Graph extraction",
    copyError: "Copy",
    errorCopied: "Error copied",
    /* 抽取丢弃：事实抽出来了却没能落地。此前完全无声——图里少了东西，没人说得出少了什么 */
    dropsChip: (n: number) => `${n} dropped`,
    dropsTitle: "Facts that did not land",
    dropsNote:
      "These were extracted from the document but blocked on the way in. " +
      "Each line says why, and how many.",
    dropsExample: "e.g.",
    dropReason: {
      attr_domain_mismatch: "Attribute on the wrong class",
      subject_not_declared: "Subject type unknown",
      attr_no_value: "Attribute had no value",
      attr_datatype: "Value did not match the datatype",
      low_confidence: "Below the confidence threshold",
      object_missing: "Relation had no object",
      malformed_item: "The model's item did not fit the schema",
      truncated_reply: "The model's reply was cut off",
    } as Record<string, string>,
    // 来源级重抽：不危险，只是费时费钱——轻确认，文案直说成本与保留项
    reExtractSource: "Re-extract",
    reExtractTitle: "Re-extract this source?",
    reExtractHint: (n: number, name: string) =>
      `All ${n} ready document${n === 1 ? "" : "s"} in “${name}” go through the extraction model again. ` +
      `Existing merges, review decisions and confirmed facts are preserved.`,
    reExtractConfirm: "Re-extract",
    queuedDocs: (n: number) => `${n} document${n === 1 ? "" : "s"} queued`,
    // 全库重建：毁灭性——打字级确认
    rebuild: "Rebuild graph",
    rebuildTitle: "Rebuild the knowledge graph?",
    rebuildHint: (docs: number, name: string) =>
      `Type “${name}” to confirm. Every entity, fact, merge and pending review in this knowledge base ` +
      `is permanently removed, then all ${docs} document${docs === 1 ? "" : "s"} are re-extracted from scratch. ` +
      `Documents, search indexes and the ontology are untouched; the decision ledger is kept.`,
    rebuildConfirm: "Rebuild permanently",
    rebuildDone: (e: number, f: number, q: number) =>
      `Cleared ${e} entities and ${f} facts · ${q} documents queued`,
    status: {
      pending: "Queued",
      parsing: "Parsing",
      indexing: "Indexing",
      embedding: "Embedding",
      ready: "Ready",
      failed: "Failed",
    },
    graphStatus: {
      none: "—",
      queued: "Queued",
      extracting: "Extracting",
      done: "Done",
      failed: "Failed",
    },
    sources: "Sources",
    allDocs: "All documents",
    uploads: "Uploads",
    addSource: "Add source",
    sourceKinds: {
      folder: "Folder",
      url: "URLs",
      rss: "RSS feed",
      api: "API",
      custom: "Custom",
    },
    sourceKindHints: {
      folder:
        "A plain folder. Select it and upload (or drag) files straight into it — nothing is watched or synced.",
      url: "Fetches the listed web pages; changed pages update the same document.",
      rss: "Subscribes to a feed; each entry becomes a document dated by its publish time.",
      api: "External systems push JSON documents here, authenticated with this source's own token.",
      custom:
        "Polls a URL you control on a schedule — your service returns JSON items and Utopia keeps them in sync.",
      memory:
        "Episodes remembered from Chat. Append-only: contradicted memories close their " +
        "validity range instead of being deleted — the timeline keeps the whole story.",
    },
    ingestGuide: "Read the ingest guide →",
    ingestGuideTitle: "Ingest interface guide",
    endpointField: "Endpoint URL",
    endpointCopied: "Endpoint URL copied",
    copyEndpoint: "Click to copy the full URL",
    // api 来源的推送状态（queued/running 不会出现，仅为类型完备）
    pushStatus: {
      never: "No pushes yet",
      queued: "—",
      running: "—",
      ok: "Push received",
      failed: "Push failed",
    },
    tokenTitle: "Source token",
    tokenWarning:
      "Anyone holding this token can push documents into this source. Rotate it if it leaks — the old token stops working immediately.",
    tokenUsage: "Send it with every push as:",
    tokenCopied: "Token copied",
    viewToken: "Token",
    rotateToken: "Rotate",
    noToken: "This source has no token yet — generate one to start pushing.",
    generateToken: "Generate token",
    close: "Close",
    authHeaderField: "Authorization header (optional, never shown again)",
    syncNow: "Sync now",
    syncStatus: {
      never: "Never synced",
      queued: "Queued",
      running: "Syncing…",
      ok: "Synced",
      failed: "Sync failed",
    },
    lastSyncAdded: (n: number) => `+${n} last sync`,
    sourceName: "Name",
    urlsField: "Page URLs (one per line)",
    feedUrl: "Feed URL",
    interval: "Sync schedule",
    intervalManual: "Manual only",
    intervalEvery: (m: number) =>
      m < 60 ? `Every ${m} min` : `Every ${m / 60} h`,
    schedule: {
      manual: "Manual",
      interval: "Interval",
      daily: "Daily",
      weekly: "Weekly",
      advanced: "Advanced",
      every: "Every",
      minutes: "minutes",
      hours: "hours",
      at: "at",
      cronPlaceholder: "Enter a cron expression",
      whatIsCron: "What is cron?",
      cronDocsUrl: "https://crontab.guru",
      daysShort: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
      dailyAt: (t: string) => `Daily ${t}`,
    },
    createSource: "Create",
    cancel: "Cancel",
    deleteSource: "Delete source",
    deleteSourceHint: "Documents are kept and move to Uploads.",
    newSourceTitle: "New source",
    iconLabel: "Icon",
    pageOf: (from: number, to: number, total: number) =>
      `${from}–${to} of ${total}`,
    syncHistory: "History",
    runNew: (n: number) => `+${n} new`,
    runUpdated: (n: number) => `~${n} updated`,
    runNothing: "no changes",
    noRuns: "No syncs recorded yet.",
    sourceSettings: "Source settings",
    editSourceTitle: "Source settings",
    saveChanges: "Save changes",
    authHeaderEditField: "Authorization header",
    authKeepHint: "Leave blank to keep the current value.",
    editKeepNote:
      "Changing what this source fetches never deletes existing documents. " +
      "Items the new configuration no longer returns are marked “Not in source” " +
      "after the next sync. Switching to a different service entirely? Create a new source instead.",
    notInSource: "Not in source",
    cleanupMissing: (n: number) => `Clean up ${n} missing`,
    cleanupTitle: "Delete missing documents",
    cleanupHint: (n: number, name: string) =>
      `${n} document${n === 1 ? "" : "s"} in “${name}” ${n === 1 ? "is" : "are"} no longer ` +
      "present in the source. Deleting removes their content and search entries permanently. " +
      "Facts already extracted into the graph remain, with their provenance.",
    cleanupConfirm: "Delete them",
    deleteSourceTitle: "Delete this source",
    deleteSourceBody: (name: string) =>
      `Type “${name}” to confirm. Documents are kept and move to Uploads; ` +
      "scheduled syncing stops.",
    dangerZone: "Danger zone",
  },
  search: {
    placeholder: "Search the knowledge base… (keyword + semantic)",
    button: "Search",
    searching: "Searching…",
    noResults: "No results found",
    chunkOf: (filename: string, seq: number) => `${filename} · section ${seq}`,
  },
  ask: {
    /* 新对话首屏问候：碑铭衬线，品牌名入句（标题不带句号） */
    greeting: "Ask Utopia what it remembers",
    emptyTitle: "Chat",
    emptyBody:
      "Converse with your knowledge base — cited answers, temporal questions, and it can remember.\nUpload documents in Library and configure a model in Settings first.",
    placeholder: "Ask anything…",
    composerHint: "Enter to send · Shift+Enter for a new line",
    scopeLabel: "Knowledge base",
    send: "Send",
    stop: "Stop",
    thinking: "Thinking…",
    newChat: "New chat",
    untitled: "Untitled",
    noConversations: "No conversations yet.",
    deleteConversation: "Delete conversation",
    deleteTitle: "Delete conversation?",
    deleteHint: (name: string) =>
      `“${name}” and its messages will be permanently removed.`,
    deleteBtn: "Delete",
    cancel: "Cancel",
  },
  graph: {
    // 还没判出类型的实体（0009）。不是一个类，是"这一格还空着"
    untyped: "Untyped",
    zoomIn: "Zoom in",
    zoomOut: "Zoom out",
    fitView: "Fit view",
    layoutForce: "Force layout",
    layoutCircular: "Circular layout",
    layoutPack: "Cluster by type",
    searchEntity: "Search entities…",
    searchInSubgraph: "Search in subgraph…",
    backToOverview: "← Full graph",
    // 顺序不是随便排的：模型没配好之前，上传的文档只会排队等着，
    // 一个实体也抽不出来。先配模型，再传文档
    emptyBody:
      "The graph is empty. Configure a chat model in Settings first, then upload documents in the Library — entities and relations are extracted automatically.",
    facts: "facts",
    noFacts: "No facts for this entity yet",
    confidence: "confidence",
    evidence: "evidence",
    noEvidence: "No evidence recorded",
    noQuote: "(no quote)",
    /* 抽取器从原文读出来的谓词，规范成了标识符。词表外的说法会被降级成
       related to，原意只在这里活着。
       **措辞不能宣称这是引文**：关系 key 只能是 [a-z0-9_]，所以中文语料里
       「采购了」出来是 purchases——说"原文说的是 purchases"是假的。
       逐字原句就在旁边的证据引文里，没丢。 */
    proposedPredicate: (p: string) => `read from the text as “${p}”`,
    inferredPredicate: "not a relation in the ontology, this is the source's wording",
    unknownPredicate: "no relation stated",
    sectionRef: (filename: string, seq: number) =>
      `${filename} · section ${seq} →`,
    fromVersion: (v: number) => `v${v}`,
    staleEvidenceHint:
      "This evidence comes from an earlier version of the document. " +
      "The document has since been updated; the fact itself is unaffected.",
    staleFactChip: "unconfirmed",
    staleFactHint:
      "All evidence for this fact comes from earlier versions of its source documents — " +
      "the current content no longer states it. It may still be true; review it under Review.",
    /* 实体修正：抽取给的是初判，人可以推翻它 */
    edit: "Edit",
    editName: "Name",
    editType: "Type",
    editSave: "Save",
    editCancel: "Cancel",
    editSaved: "Entity updated",
    editEmptyName: "Name cannot be empty",
    /* 同名不是错误——两个张伟可以并存。只提示，不阻断 */
    sameNameNote: (n: number) =>
      n === 1
        ? "One other entity shares this name."
        : `${n} other entities share this name.`,
    sameNameHint: "If they are the same thing, merge them under Review.",
    viewRelations: "Relations",
    viewTimeline: "Timeline",
    /* 第三视图：记录时间轴——不是"事情何时发生"，而是"我们何时这么认为" */
    viewHistory: "History",
    historyHint: "How this entity's record changed — and who changed it.",
    historyEmpty: "Nothing recorded for this entity yet.",
    historyKind: {
      asserted: "Recorded",
      corrected: "Interval corrected",
      rejected: "Withdrawn",
      /* 并入另一条断言：内容一字未少，不是撤回 */
      merged: "Merged into an existing fact",
      /* 改的是节点上的类,一条事实都没动 */
      retyped: "Type changed",
      retype_reverted: "Type change undone",
    } as Record<string, string>,
    historyEngine: "engine",
    /* 有效区间的变化：修正后区间闭合到某个时点 */
    historyClosedAt: (t: string) => `closed at ${t}`,
    historyFrom: (t: string) => `from ${t}`,
    historyOngoing: "open-ended",
    historicalNote: (n: number) =>
      `${n} past fact${n === 1 ? "" : "s"} not shown — see Timeline →`,
    undated: "Undated",
    timelineEmpty: "No dated facts yet.",
    lastConfirmed: (d: string) => `confirmed ${d}`,
    correctedHint:
      "This interval was closed by reconciliation (automatic succession or a review decision), " +
      "not stated verbatim in a document. The superseded assertion remains in the ledger.",
    ongoing: "now",
    /* 必须跟 ongoing 看得出区别：混淆这两个正是迁移 0046 要修的东西——
       原文说 "former CEO"，界面却显示 now */
    endedUnknown: "ended, date unknown",
    stats: (n: number, e: number, active: number) =>
      `${n} entities · ${e} facts · ${active} active`,
    stabilizing: "Stabilizing layout",
    allTime: "All time",
    nowBtn: "Now",
    play: "Play timeline",
    pause: "Pause",
  },
  doc: {
    backToLibrary: "← Back to Library",
    sections: "sections",
    section: "Section",
    citedHere: "← cited here",
    loading: "Loading…",
    extracted: "Extracted",
    ongoing: "now",
  },
  settings: {
    title: "System settings",
    tabModels: "Models",
    tabMembers: "Users",
    tabKbs: "Knowledge bases",
    tabDeployment: "Deployment",
    newUser: "Create user",
    initialPassword: "Initial password (min. 8 characters)",
    createUserBtn: "Create",
    searchUsers: "Filter by name or email…",
    deployment: {
      openReg: "Allow self-registration",
      openRegHint:
        "When off, the sign-up form is closed and only admins can create accounts here.",
      workers: "Background workers",
      workersHint:
        "An outer ceiling on how many jobs run at once (1–256), there to stop work piling up " +
        "without bound. The real throttle is the per-model limit below, so keep this comfortably " +
        "above the sum of those. Takes effect immediately.",
      workersApply: "Apply",
      /* 真正的节流：约束来自供应商的速率限制，而那是按模型算的 */
      modelConcurrency: "Model concurrency",
      modelConcurrencyHint:
        "How many calls a model will take at once. The limit that matters belongs to the " +
        "provider and is per model — a local Ollama may manage two, a hosted API fifty. " +
        "Background work (extraction, resolution, indexing) waits for a slot; chat and search " +
        "never do. Takes effect immediately.",
      /* 部署级默认值：新建库时用。名字刻意不叫"系统语言" */
      ontologyLang: "Default ontology language",
      ontologyLangHint:
        "The language new knowledge bases start their ontology in — class descriptions go " +
        "into the extraction prompt, so this follows the documents you expect, not the " +
        "interface. Each knowledge base can change its own afterwards. " +
        "Interface language is a per-reader choice in the account menu.",
      modelDefault: "Default",
      modelReset: "Reset",
      modelResetHint:
        "Drop this model's own limit and fall back to the default.",
    },
    datasources: {
      tab: "Data sources",
      title: "Data sources",
      hint:
        "Read-only database connections for asking questions about your data in Chat. " +
        "Register connections here; each knowledge base mounts the ones it may query.",
      name: "Name",
      connString: "Connection string (postgres://user:pass@host:5432/db)",
      add: "Add data source",
      test: "Test",
      testOk: "Connected",
      testFail: "Failed",
      neverTested: "Untested",
      remove: "Remove",
    },
    kbs: {
      hint:
        "Every knowledge base in this deployment. Open ones are readable by all members; " +
        "restricted ones are invite-only. Creating a knowledge base is an admin action — " +
        "members switch between them from the top bar.",
      defaultChip: "Default",
      newKb: "New knowledge base",
      packsLabel: "Bundled ontologies",
      packsHint: "Optional. Packs declare direction, so subject and object cannot come out reversed. More can be imported later.",
      packsNone: "None — start from the ten seed relations",
      packsCount: (c: number, p: number) => `${c} classes · ${p} properties`,
      name: "Name",
      description: "Description",
      visibility: "Visibility",
      visOpen: "Open — everyone in this deployment",
      visRestricted: "Invited only",
      create: "Create",
      openSettings: "Settings",
      docs: (n: number) => `${n} docs`,
    },
    modelsIntro:
      "OpenAI-compatible protocol — DeepSeek, Qwen, GLM, Ollama, vLLM all work. Fully on-prem friendly.",
    chatModel: "Chat model",
    embedModel: "Embedding model (optional, enables semantic search)",
    baseUrl: "Base URL",
    model: "Model",
    apiKey: "API key",
    keyConfigured: "(configured — leave blank to keep)",
    save: "Save",
    saving: "Saving…",
    saved: "Saved",
    test: "Test connection",
    testing: "Testing…",
    chatLabel: "Chat",
    embedLabel: "Embedding",
    ok: (reply: string) => `Connected (${reply})`,
    okDim: (dim: number) => `Connected (dim ${dim})`,
  },
  ontology: {
    title: "Ontology",
    hint: "Classes & properties",
    tabClasses: "Classes",
    tabProperties: "Properties",
    newClass: "New class",
    newSubClass: "+ Sub-class",
    newProperty: "New property",
    filter: "Filter…",
    missesShort: "Unmatched",
    instances: "Instances",
    instanceFacts: (n: number) => `${n} facts`,
    description: "Description",
    descriptionHint:
      "Guides the extractor: what belongs here, with a couple of examples. Fed straight into the extraction prompt.",
    overviewHint:
      "The schema your extractor follows. Select a class or property on the left to edit it, or add new ones with the + buttons.",
    overviewStats: (c: number, p: number) => `${c} classes · ${p} properties`,
    attributes: "Attributes",
    attributesHint:
      "Literal-valued fields of this class (a person's salary, a contract's amount). Extracted with evidence and history, like any fact.",
    newAttribute: "New attribute",
    attrDatatype: "Value type",
    attrUnit: "Unit",
    attrUnitHint: "optional — e.g. CNY, %",
    attrSingle: "Single-valued — a new value closes the previous one",
    datatypeNames: {
      text: "Text",
      number: "Number",
      date: "Date",
      bool: "Yes / no",
    } as Record<string, string>,
    cancel: "Cancel",
    key: "Key",
    label: "Label",
    color: "Color",
    shapeColor: "Shape & color",
    parent: "Parent class",
    noParent: "(top level)",
    /* 多父时左栏只能画一处，说明画在哪一支下 */
    primaryParentHint: "Shown in the tree under the first one.",
    /* 类型签名。措辞要说清它是引导不是闸门——本体写错时模型仍可覆盖 */
    signature: "Type signature",
    signatureHint:
      "Which classes this relation connects. It goes into the extraction prompt as a hint, " +
      "not a gate: it steers the model as it writes, and the text still wins when the " +
      "ontology is wrong.",
    domainLabel: "Subject",
    rangeLabel: "Object",
    anyType: "Any type",
    searchTypes: "Search classes…",
    temporal: "Temporal semantics",
    temporalState: "State (has interval)",
    temporalEvent: "Event (point in time)",
    temporalEternal: "Eternal (timeless)",
    functional: "Functional (single value at a time)",
    inverseFunctional: "Inverse functional (one subject per object at a time)",
    usage: (n: number) => `${n} in use`,
    builtin: "built-in",
    save: "Save",
    delete: "Delete",
    deleteBlocked: "In use — cannot delete",
    /* ---- OWL / RDFS 导入 ---- */
    importShort: "Import",
    importTitle: "Import an ontology",
    importHint:
      "Load an OWL or RDFS file (.owl, .rdf, .ttl). Classes and properties are matched by IRI, so re-importing a newer version of the same vocabulary updates what it already created instead of duplicating it.",
    importPick: "Choose file",
    importChange: "Choose another",
    importReading: "Reading…",
    importApplying: "Importing…",
    importApply: "Import",
    importCancel: "Cancel",
    importParsed: (fmt: string, triples: number) =>
      `${fmt === "rdfxml" ? "RDF/XML" : "Turtle"} · ${triples.toLocaleString()} triples`,
    importNothing:
      "Nothing to import — no classes or properties found in this file.",
    /* 计划三列：新建 / 更新 / key 被占 */
    importWillCreate: (n: number) => `${n} new`,
    importWillUpdate: (n: number) => `${n} updated`,
    importKeyTaken: (n: number) => `${n} skipped`,
    importClasses: "Classes",
    importRelations: "Relations",
    importAttributes: "Attributes",
    /* 属性还落不了库：它们要 domain，而 domain 要等类先建好并解析 IRI */
    importAttributesLater:
      "Parsed, but not created yet — attributes need a class to hang from, which lands in the next step.",
    /* 预览必须警告的第一件事：functional 会让时序引擎自动关掉旧事实。
       part_of 那次一个错误的唯一性声明造了 59 条假冲突 */
    warnFunctional: (n: number) =>
      `${n} ${n === 1 ? "relation declares" : "relations declare"} itself functional`,
    warnFunctionalBody:
      "A functional relation may hold one value at a time, so a new fact automatically closes the previous one. When the vocabulary claims uniqueness your data does not keep, that shows up as a queue of conflicts. Review these after importing.",
    /* 第二件事：description 逐字进抽取提示词，没有它的类抽得明显差 */
    warnNoDescription: (n: number) =>
      `${n} ${n === 1 ? "class arrives" : "classes arrive"} with no description`,
    warnNoDescriptionBody:
      "A class description goes verbatim into the extraction prompt — it is the only thing telling the model what belongs there. Write one for these, or they will quietly under-extract.",
    /* key 撞了：报告不解决。自动加后缀会让下次重导入认不出自己上次建的是哪个 */
    warnKeyTaken: (n: number) =>
      `${n} ${n === 1 ? "key is" : "keys are"} already taken`,
    warnKeyTakenBody:
      "Something else already holds this key under a different identity. These are left alone — rename the existing one first if you want the imported version instead.",
    /* 占位者没有 IRI = 这库里手工建的或内置的，那句话比一个空 IRI 有用 */
    importTakenBy: (iri: string | null) =>
      iri
        ? `taken by ${iri}`
        : "taken by an entry defined in this knowledge base",
    /* 出现过但今天不投影的公理，按名字与次数列出——"暂未投影"不是"已跳过" */
    importUnprojected: "Not projected yet",
    importUnprojectedBody:
      "Axioms this file uses that Utopia does not consume yet. Nothing is lost: the source file is stored as uploaded, so a later version can project them.",
    importDone: (created: number, updated: number) =>
      `Imported — ${created} classes created, ${updated} updated.`,
    importHistory: "Previous imports",
    importNoHistory: "No imports yet.",
    importBy: (who: string, when: string) => `${who} · ${when}`,
    importSize: (bytes: number) =>
      bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(0)} KB`,
    misses: "Unmatched from extraction",
    missesHint:
      "The extractor produced these outside your ontology (they fell back to concept / related to). They are signals for extending the ontology.",
    dismiss: "Dismiss",
    dismissed: (n: number) => `Dismissed (${n})`,
    /* 数字是**忽略之后**还在涨的那个——这一行的全部意义就在于此：
       当初"只出现过一次"的判断依据可能早就不成立了 */
    dismissedHint:
      "Still counted, but kept out of suggestions. If one has grown since you dismissed it, restore it.",
    restore: "Restore",
    suggest: "Suggest with AI",
    suggesting: "Analyzing…",
    noMisses: "No unmatched types — the ontology covers your corpus.",
    approve: "Add",
    /* 映射那一档的按钮。刻意不叫 Add——它不加东西，本体里已经有了。
       两个按钮都写 Add 的话，"已经有了"这件事在界面上就消失了 */
    mapOver: "Use existing",
    /* 影响面：采纳一个提案会把多少条无谓词事实认过去。
       没有这一句，"Add" 只是凭空多一个空关系 */
    willRemap: (n: number) =>
      n === 1 ? "reclassifies 1 fact" : `reclassifies ${n} facts`,
    adopted: (n: number) =>
      n === 1
        ? "Added — 1 fact reclassified"
        : `Added — ${n} facts reclassified`,
    /* 一部分值换不动这个类型就没被改写。只报改写了多少条是报喜不报忧 */
    adoptedPartly: (moved: number, left: number) =>
      `Added — ${moved} reclassified, ${left} left behind (value did not fit the type)`,
    /* 撤销：采纳改写了成批事实，没有回头路的话没人敢点第一下 */
    undoAdopt: (key: string, n: number) =>
      `${key} added, ${n} fact${n === 1 ? "" : "s"} reclassified`,
    undoAdoptBtn: "Undo",
    reverted: (n: number) =>
      n === 1 ? "Reverted — 1 fact restored" : `Reverted — ${n} facts restored`,
    undoKeepsRelation: "The relation stays; only the facts move back.",
    /* 撤销要二次确认：它一次改回成批事实 */
    undoTitle: "Undo this ontology change?",
    undoHint: (n: number) =>
      `${n} fact${n === 1 ? "" : "s"} will go back to “related to”. The relation itself stays — ` +
      `nothing is deleted, and you can adopt it again later.`,
    undoConfirm: "Undo",
    undoCancel: "Keep",
    /* 自动扩本体的通知：默认开启的前提是它的动作可见且可退。
       只记在审计台账里不算可见——那是查证用的，不是通知用的 */
    autoRanTitle: "Utopia extended this ontology from your documents",
    autoRanBody: (rels: string[], facts: number) =>
      `Added ${rels.join(", ")} · ${facts} fact${facts === 1 ? "" : "s"} reclassified`,
    autoRanOff: "Turn this off in knowledge base settings.",
    /* 批量：常见情形是"这些都对"，一条条点是把一个决定拆成八个 */
    addAll: (n: number) => `Add all ${n}`,
    addingAll: "Adding…",
    addAllLabel: "batch",
    addAllPartial: (keys: string[]) =>
      `Some could not be added: ${keys.join(", ")} — the rest went through.`,
    proposals: "AI proposals",
    keyHint: "lowercase_snake_case",
  },
  review: {
    title: "Review",
    hint: "Duplicates & low-confidence facts",
    tabQueue: "Queue",
    tabHistory: "History",
    empty: "Nothing to review — the graph is clean.",
    historyEmpty: "No merges yet.",
    // 左栏分类导航
    railDuplicates: "Duplicates",
    railConflicts: "Conflicts",
    railUnconfirmed: "Unconfirmed",
    railLowConfidence: "Low confidence",
    railMappings: "Semantic layer",
    railDecisions: "Decisions",
    railMerges: "Merges",
    categoryEmpty: "This queue is clear.",
    // 决策台账
    decisionsTitle: "Decisions",
    decisionsHint:
      "Every review decision, by whom and when — snapshots taken at decision time, kept even after the underlying fact is gone.",
    decisionsEmpty: "No decisions recorded yet.",
    aiActor: "AI adjudicator",
    decisionActions: {
      "review.merge": "Merged",
      "review.keep": "Kept apart",
      "fact.confirm": "Confirmed",
      "fact.reject": "Rejected",
      "fact.close": "Closed",
      "conflict.close_old": "Closed old",
      "conflict.keep_both": "Kept both",
      "conflict.reject_new": "Rejected new",
      "merge.revert": "Reverted merge",
      "merge.manual": "Merged manually",
    } as Record<string, string>,
    /** 升格给人裁决的原因。服务端存 code（可选 |detail），措辞在这里 */
    escalated: {
      escalate_no_model: "No chat model — the adjudicator could not run",
      escalate_no_verdict: "The adjudicator returned no verdict",
      escalate_entity_changed: "The entity changed while being adjudicated",
      escalate_unsure: "The adjudicator was not confident enough",
      /* 名字互相包含：等值召回看不见，简称会静默变成第二个实体 */
      contains: "One name contains the other",
      ambiguous_name: "Same name, context did not settle it",
      type_drift: "Same name arrived under a different type",
      auto_merged: "Merged by the AI adjudicator",
      kept_apart: "The AI adjudicator judged these different",
    } as Record<string, string>,
    duplicates: "Possible duplicates",
    duplicatesHint:
      "Same name, different context. The AI adjudicates clear cases in the background; the rest wait for you. Merging is always reversible.",
    stageAdjudicating: "AI adjudicating",
    stageHuman: "Needs your decision",
    similarity: (pct: number) => `${pct}% context similarity`,
    factsCount: (n: number) => `${n} facts`,
    noFacts: "No recorded facts",
    merge: "Merge",
    keep: "Keep separate",
    lowConfidence: "Low-confidence facts",
    mappings: "Semantic layer",
    mappingsHint:
      "Proposed mappings from a business concept to how it is computed. Confirm one and Ask uses it instead of guessing from the schema.",
    mappingDerived: "derived",
    lowConfidenceHint:
      "Extracted with confidence below 75%. Confirm to trust, reject to remove from the graph (the ledger keeps the record).",
    confirm: "Confirm",
    reject: "Reject",
    confidence: (pct: number) => `${pct}%`,
    mergeHistory: "Merge history",
    mergedBy: (name: string) => `by ${name}`,
    mergedByAi: "by AI adjudicator",
    revert: "Revert",
    reverted: "Reverted",
    ongoing: "now",
    conflicts: "Temporal conflicts",
    conflictsHint:
      "Two facts claim the same single-valued relation. Clear successions close automatically; " +
      "these need a human call. Closing is reversible through the ledger.",
    conflictReason: {
      no_time: "new fact has no date",
      simultaneous: "same start date",
      low_confidence: "low confidence",
    } as Record<string, string>,
    conflictVs: "vs",
    conflictSince: (d: string) => `since ${d}`,
    closeOld: "Close old",
    closeOldAt: (d: string) => `Close old at ${d}`,
    keepBoth: "Keep both",
    rejectNew: "Reject new",
    closeAtPlaceholder: "YYYY-MM-DD",
    unconfirmed: "No longer stated",
    unconfirmedHint:
      "Every source that stated these facts has since been updated without them. " +
      "Nothing is deleted automatically — absence isn't negation. Reject extraction " +
      "errors, or close a fact that genuinely ended (pick the date it ended).",
    closeFact: "Close",
    closeFactAt: (d: string) => `Close at ${d}`,
  },
  /** 通用组件文案（SearchSelect 等） */
  ui: {
    noMatches: "No matches",
    keepTyping: (n: number) => `${n} more — keep typing to narrow down`,
  },
  kbset: {
    title: "Knowledge base settings",
    general: "General",
    members: "Members",
    /* 自动扩本体开关。说明必须讲清关掉之后失去的**只是**代劳，不是留意——
       否则用户会以为关掉它就看不到未匹配的信号了 */
    autoExtend: "Extend the ontology automatically",
    autoExtendNote:
      "When extraction meets a relation this ontology does not have, add it and reclassify the " +
      "facts that were waiting for it. Every change is listed and can be undone. Turning this " +
      "off does not stop Utopia from noticing — the phrases still collect under Unmatched, they " +
      "just wait for you to approve them.",
    /* 语料语言。措辞要把"这不是界面语言"讲清楚，否则一定有人当成界面开关 */
    ontologyLang: "Language of this ontology",
    ontologyLangNote:
      "Which language class and relation descriptions are written in. Those go straight " +
      "into the extraction prompt, so the reader is the model while it reads your documents — " +
      "match your documents, not your interface. Changing this does not rewrite what is " +
      "already here; it decides the language of descriptions written from now on.",
    defaultOpenLabel: "Open to everyone",
    defaultOpenNote:
      "This is the deployment's default knowledge base, so visibility is locked: every member " +
      "gets at least viewer access here, which guarantees nobody signs in to an empty screen. " +
      "It can't be deleted for the same reason. To give someone more than viewing, grant a " +
      "role under Members; for a private space, create a separate knowledge base and set it " +
      "to Restricted.",
    data: "Data",
    dataHint:
      "Mounted read-only databases this knowledge base may query from Chat. " +
      "Mounting ingests the database schema so the assistant knows the tables.",
    dataMount: "Mount",
    dataUnmount: "Unmount",
    dataSyncSchema: "Refresh schema",
    dataSchemaSynced: (n: number) => `Schema ingested (${n} tables)`,
    dataExplore: "Explore mappings",
    dataExploreHint:
      "An agent reads the schemas and proposes metric/dimension definitions — review them in Review before Chat uses them.",
    dataExploreQueued:
      "Exploration queued — proposals will appear in Review shortly.",
    dataNone: "No data sources mounted.",
    dataNoneAvailable:
      "No data sources registered yet — ask a deployment admin to register one.",
    dataNewConn: "Register a new connection",
    activity: "Activity",
    activityHint:
      "Who changed what in this knowledge base. Pure audit — records are append-only.",
    activityEmpty: "Nothing recorded yet.",
    deletedUser: "a removed user",
    auditActions: {
      "entity_type.created": "created entity type",
      "entity_type.updated": "updated entity type",
      "entity_type.deleted": "deleted an entity type",
      "relation_type.created": "created relation type",
      "relation_type.updated": "updated relation type",
      "relation_type.deleted": "deleted a relation type",
      "kb.updated": "updated knowledge base settings",
      "kb.member_set": "set a member role",
      "kb.member_removed": "removed a member",
      "source.created": "created source",
      "source.updated": "updated source",
      "source.deleted": "deleted a source",
      "document.deleted": "deleted document",
      "ontology.imported": "imported an ontology",
    } as Record<string, string>,
    membersHintOpen:
      "Everyone in this deployment can already read this knowledge base, so there is no " +
      "viewer role to grant — list someone here only to give them write access. " +
      "Deployment admins always have it.",
    membersHintRestricted:
      "Only the people listed here can see this knowledge base, and their role decides " +
      "what they can change. Deployment admins always have access.",
    addMember: "Add…",
    roles: { viewer: "Viewer", editor: "Editor", admin: "Admin" },
    remove: "Remove",
    noMembers: "No per-KB roles set.",
    noWriters: "Nobody has been given write access yet.",
    save: "Save",
    saved: "Saved",
    danger: "Danger zone",
    deleteKb: "Delete this knowledge base",
    deleteHint: (name: string) =>
      `Type “${name}” to confirm. Documents, graph and sources are permanently removed.`,
    deleteBtn: "Delete permanently",
    deleteRowTitle: "Delete this knowledge base",
    deleteRowHint: "Documents, graph and sources are removed permanently.",
    deleteRowBtn: "Delete",
  },
  members: {
    title: "Deployment users",
    systemAdmin: "System admin",
    remove: "Remove",
    deactivate: "Deactivate",
    deactivateHint:
      "Cuts off access everywhere — sign-in and any token already issued. What they did stays attributed to them.",
    deactivateConfirm: (name: string) =>
      `Deactivate ${name}? They lose access everywhere. Their past decisions stay on record.`,
    pickUser: "Select a user to add…",
    add: "Add",
    roles: {
      owner: "Owner",
      admin: "Admin",
      editor: "Editor",
      viewer: "Viewer",
    },
  },
};

/** 语言包的结构契约。其余语言包写成 `const zh: Strings = {…}`，漏一条即编译失败 */
export type Strings = typeof en;
