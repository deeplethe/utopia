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
    attr_has_no_link:
      "An attribute has no inverse and no super-property — its value is a literal, not something to point back from.",
    link_target_is_attr:
      "Pick a relation, not an attribute — an attribute's value is a literal.",
    sub_property_self: "A relation cannot be its own super-property.",
    unknown_relation: "That relation is not in this knowledge base.",
    entity_name_required: "Name cannot be empty.",
    entity_name_too_long: "Name is too long — 100 characters at most.",
    unknown_entity_type: "That class is not in this ontology.",
    nothing_to_update: "Nothing to change.",
    self_merge: "An entity cannot be merged into itself.",
    close_at_required:
      "Pick the date this fact ended — the new one does not say when it started.",
    empty_query: "Type something to search for.",
    no_data_sources: "No databases are mounted on this knowledge base.",
    // 授权是逐工作区的（0014）：源没授权给本库所属的工作区
    source_not_granted:
      "This data source is not granted to this workspace. Ask a deployment admin to grant it in System settings → Data sources.",
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
    inference_off:
      "Materialized inference is off for this knowledge base. Turn it on in Settings.",
    bad_resolution: "That is not a valid decision.",
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
      "data_source.schema_sync_failed": {
        title: "A data source is mounted, but its schema is not",
        hint: "Ask cannot see which tables exist, so it will guess column names. Check the connection, then use Refresh schema.",
      },
      "llm.unreachable": {
        title: "The model endpoint gave no usable answer",
        hint: "Extraction and embedding are stopped. Check the endpoint URL in system settings.",
      },
      "llm.rate_limited": {
        title: "The model endpoint is rate limiting us",
        hint: "Documents were retried and still turned away, so some are missing facts. Lower model concurrency in system settings, or raise the quota on the account.",
      },
      "llm.out_of_credit": {
        title: "The model account cannot pay for requests",
        hint: "Extraction and embedding are stopped and will not resume on their own. Top up the account, or point system settings at an endpoint that can serve.",
      },
    } as Record<string, { title: string; hint: string } | undefined>,
    // 没见过的 kind 也要能显示：新告警源上线时前端可能还没更新
    unknownKind: (kind: string) => kind,
  },

  kbScope: {
    deniedTitle: "You don't have access to this knowledge base",
    deniedBody:
      "The link points at a base you can't open. Ask whoever shared it to grant you access, or pick one of your own.",
    missingTitle: "This knowledge base is gone",
    missingBody:
      "It was deleted, or the link was mistyped. Your own bases are listed below.",
    myKbs: "My knowledge bases",
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
    anyStatus: "Any extraction state",
    statusFailed: "Failed",
    statusDone: "Extracted",
    statusQueued: "Queued",
    statusExtracting: "Extracting",
    statusNone: "Not extracted",
    retryFailed: (n: number) => `Retry ${n} failed`,
    retryQueued: (n: number) => `${n} queued for extraction`,
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
      domain_mismatch:
        "The subject does not fit the relation, and swapping would not help",
      not_an_entity_name: "That name is a sentence, not a thing",
      direction_corrected:
        "Subject and object were swapped to match the signature",
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
      github_issues: "GitHub issues",
      jira_issues: "Jira issues",
      s3: "S3 / MinIO",
      azure_blob: "Azure Blob",
      gcs: "Google Cloud Storage",
    },
    sourceKindHints: {
      folder:
        "A plain folder. Select it and upload (or drag) files straight into it — nothing is watched or synced.",
      url: "Fetches the listed web pages; changed pages update the same document.",
      rss: "Subscribes to a feed; each entry becomes a document dated by its publish time.",
      jira_issues:
        "Syncs a Jira project's issues. **Each ticket, together with its field-level change " +
        "history**, becomes one document — what changed from what to what, and when. " +
        "One call fetches everything; no per-ticket round trips.",
      github_issues:
        "Syncs a repository's issues. **Each ticket, together with its state history**, becomes " +
        "one document — when it was opened, closed, relabelled, reassigned, all dated. " +
        "Without a token GitHub allows only 60 requests an hour.",
      s3:
        "Reads documents out of an S3 bucket, or anything speaking the same protocol " +
        "(MinIO, Ceph, R2). **Leave the endpoint empty for AWS**; fill it in for a " +
        "self-hosted one. Each object becomes a document dated by its last modification.",
      azure_blob:
        "Reads documents out of an Azure Blob container. Leave the endpoint empty for " +
        "Azure itself; fill it in for Azurite or a gateway.",
      gcs:
        "Reads documents out of a Google Cloud Storage bucket. The service account JSON " +
        "goes in whole — there is no file on the server to point at.",
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
    repoField: "Repository (owner/name)",
    jiraUrlField: "Jira site URL",
    jiraProjectField: "Project key",
    s3BucketField: "Bucket",
    s3PrefixField: "Prefix (optional — without one the whole bucket is read)",
    s3EndpointField: "Endpoint (leave empty for AWS S3)",
    s3RegionField: "Region",
    s3KeyField: "Access key ID",
    s3SecretField: "Secret access key",
    azAccountField: "Storage account name",
    azKeyField: "Account key",
    gcsKeyField: "Service account JSON (paste the whole file)",
    tokenField: "GitHub token (optional, never shown again)",
    includePullRequests: "Treat pull requests as tickets too",
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
    searchConversations: "Search chats",
    moreActions: "More",
    rename: "Rename",
    copyTitle: "Copy title",
    deleteTitle: "Delete conversation?",
    deleteHint: (name: string) =>
      `“${name}” and its messages will be permanently removed.`,
    deleteBtn: "Delete",
    cancel: "Cancel",
  },
  graph: {
    // 还没判出类型的实体（0009）。不是一个类，是"这一格还空着"
    untyped: "Untyped",
    legendMore: (n: number) => `All ${n} classes`,
    nodeBudget: "How many entities to draw",
    nodeBudgetMore: "Draw more",
    nodeBudgetLess: "Draw fewer",
    legendSearch: "Filter classes",
    legendNone: "No class matches",
    legendOnly: "Only",
    legendShowAll: (n: number) => `Show all (${n} hidden)`,
    legendAllHint:
      "Every class on screen, most common first. Click to show or hide.",
    searchMore: (n: number) => `${n} more — load 20`,
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
    inferredPredicate:
      "not a relation in the ontology, this is the source's wording",
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
    mergeInto: "Merge in",
    mergeIntoHint:
      "Fold that entity into this one. Its facts move here; merges can be reverted.",
    mergeConfirm: (from: string, into: string) =>
      `Merge “${from}” into “${into}”? Its facts move here. You can revert this from Review.`,
    viewRelations: "Relations",
    viewTimeline: "Timeline",
    /* 第三视图：记录时间轴——不是"事情何时发生"，而是"我们何时这么认为" */
    viewHistory: "History",
    viewDerived: "Derived",
    derivedEdges: (n: number) => `${n} derived`,
    derivedHint:
      "Edges no one asserted — the engine worked them out from axioms your ontology declares. Each one shows the premises it came from.",
    derivedNoProof: "The premises are gone.",
    derivedPanel: "Inference",
    derivedRunAsk: "Re-run inference for the whole base?",
    derivedRunGo: "Run",
    derivedRunCancel: "Cancel",
    derivedCountLabel: "Edges derived",
    derivedStateLabel: "Schedule",
    derivedLastLabel: "Last run",
    derivedOn: (mins: number) => `every ${mins} min`,
    derivedOff: "off",
    derivedNever: "never",
    derivedAgo: (mins: number) =>
      mins < 1
        ? "just now"
        : mins < 60
          ? `${mins} min ago`
          : `${Math.round(mins / 60)} h ago`,
    derivedRun: "Run now",
    derivedRunning: "Running…",
    derivedNoChange: "Nothing changed.",
    derivedChanged: (added: number, gone: number) =>
      `${added} added · ${gone} retracted`,
    derivedCapped: (n: number) => `${n} predicate(s) not closed fully`,
    close: "Close",
    // 派生边靠哪条规则来的。**四种都要有**——查不到的会退回原始 kind 串，
    // 而那对读的人没有意义
    ruleNames: {
      transitive: "transitive",
      symmetric: "symmetric",
      inverse: "inverse",
      sub_property: "sub-property",
    } as Record<string, string | undefined>,
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
    stats: (n: number, e: number, active: number | null) =>
      `${n} entities · ${e} facts${active === null ? "" : ` · ${active} active`}`,
    /** 画布只画度数最高的一批。**说清楚画了多少、共多少**——从前这里写的是
     *  上限，一个上万实体的库右上角永远是 150 */
    statsCapped: (
      shown: number,
      total: number,
      shownE: number,
      totalE: number,
      active: number | null,
    ) =>
      `showing ${shown} of ${total} entities · ${shownE} of ${totalE} facts${active === null ? "" : ` · ${active} active`}`,
    cappedHint: (shown: number, total: number) =>
      `The canvas draws the ${shown} best-connected entities of ${total}. Search to reach the rest.`,
    stabilizing: "Stabilizing layout",
    scrubUnitHint: "Step size for playback and for each bar",
    scrubUnitYear: "Yr",
    scrubUnitMonth: "Mo",
    scrubUnitDay: "Dy",
    scrubBarMerged: (n: number) => `each bar covers ${n} steps`,
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
      grants: "Available to",
      grantsHint:
        "Which workspaces may use this source. **Once granted, KB admins in those workspaces choose whether to mount it** — " +
        "this controls what they can reach, not what they have mounted.",
      grantsNone:
        "Not granted to any workspace — no knowledge base can mount it.",
      grantAdd: "Grant a workspace…",
      grantRevoke: "Revoke",
      grantRevoked: (n: number) =>
        n === 0
          ? "Revoked."
          : `Revoked, and unmounted it from ${n} knowledge base(s).`,
    },
    kbs: {
      hint:
        "Every knowledge base in this deployment. Open ones are readable by all members; " +
        "restricted ones are invite-only. Creating a knowledge base is an admin action — " +
        "members switch between them from the top bar.",
      defaultChip: "Default",
      newKb: "New knowledge base",
      packsLabel: "Bundled ontologies",
      packsHint:
        "Optional. Packs declare direction, so subject and object cannot come out reversed. More can be imported later.",
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
    refineShort: "Refine types",
    refineTitle: "Refine types",
    refineHint:
      "Entities whose class is roughly right but not the most specific one available. Look first, then apply — retyping does not appear on any timeline, so this is the only place you get to see it before it happens.",
    refinePreview: "Look first",
    refineLooking: "Looking…",
    refineRun: "Run and apply",
    refineRunning: "Running…",
    refineNothing: "Nothing to refine.",
    refineCandidates: (n: number) => `${n} entities would be considered`,
    refineNoCandidates: "Retrieval found no class for this one.",
    refineModelSays: (t: string) => `the model called it “${t}”`,
    refineRetyped: (n: number) => `${n} retyped automatically`,
    refineUndo: "Undo this batch",
    refineUndone: (n: number) => `${n} put back`,
    refineForReview: (n: number) => `${n} need your call`,
    refineCrossesAxis: "different axis",
    refineApprovePair: "Approve this class pair",
    refineLeftAlone: (n: number) => `${n} left alone`,
    refineTopCandidate: (c: string) => `closest class was ${c}`,
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
    disjoint: "Cannot also be",
    disjointHint:
      "Classes nothing can belong to at the same time. A Person is not an Organisation. The consistency check uses this to find classes that can never have an instance.",
    noDisjoint: "No class excluded",
    disjointWithParent:
      "This class inherits from a class it says it cannot be — nothing could ever satisfy it.",
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
    axioms: "Axioms",
    axiomsHint:
      "What this relation guarantees. These are not descriptions — they change what the system does: the temporal engine closes old values, and the reasoning engine adds edges to the graph.",
    functionalHint:
      "One subject, one value at a time. A new value closes the old one.",
    inverseFunctionalHint: "One object, one subject. A project has one lead.",
    transitive: "Transitive",
    transitiveHint: "A→B and B→C means A→C. The engine will add those edges.",
    symmetric: "Symmetric",
    symmetricHint: "A→B means B→A. The engine will add the other direction.",
    asymmetric: "Asymmetric",
    asymmetricHint:
      "A→B rules out B→A. Both directions get reported as a contradiction.",
    irreflexive: "Irreflexive",
    irreflexiveHint: "Nothing can point at itself through this relation.",
    axiomConflict:
      "Symmetric and asymmetric together hold only for a relation with no facts at all — one of the two is wrong.",
    noLink: "None",
    inverseOf: "Inverse",
    inverseOfHint:
      "The relation that says the same thing the other way round. Declare it on one side only — the other direction follows.",
    subPropertyOf: "Super-property",
    subPropertyOfHint:
      "The broader relation this one is a special case of. Stating the specific one also states the broader one.",
    linkMeansInverse: (p: string, q: string) =>
      `A ${p} B also means B ${q} A.`,
    linkMeansSuper: (p: string, q: string) => `A ${p} B also means A ${q} B.`,
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
  mapping: {
    title: "Data mapping",
    hint: "What business concepts point at in the database, and how they are computed. Ask only answers using confirmed definitions.",
    tabDefinitions: "Definitions",
    tabSources: "Data sources",
    filterAll: "All",
    filterProposed: "Pending",
    filterConfirmed: "Confirmed",
    filterRejected: "Rejected",
    searchPlaceholder: "Search concept, source or table…",
    total: (n: number) => `${n} total`,
    range: (from: number, to: number, total: number) =>
      `${from}–${to} of ${total}`,
    prev: "Previous",
    next: "Next",
    empty:
      "No definitions yet. Mount a data source, then run Explore to have an agent propose a first batch.",
    emptyFiltered: "No definitions match.",
    rejectedHint:
      "Rejected ones are listed too — otherwise “why was this concept never mapped?” has no answer.",
    colConcept: "Concept",
    colSource: "Source",
    colDefinition: "How it is computed",
    colStatus: "Status",
    derivedBadge: "Derived",
    noDefinition: "(empty)",
    approve: "Confirm",
    reject: "Reject",
    edit: "Edit",
    editTitle: "Revise definition",
    fieldTable: "Table",
    fieldExpr: "Expression",
    fieldSql: "SQL",
    fieldUnit: "Unit",
    fieldSummary: "Summary",
    fieldDerived: "Derived metric (computed, not a column)",
    save: "Save",
    cancel: "Cancel",
    needOne: "Fill in at least one of table, expression or SQL.",
    history: "Revision history",
    historyHint:
      "A full snapshot of the version before each change. Kept so “how was this number computed last quarter?” has an answer.",
    historyEmpty: "Never revised.",
    historyBy: (who: string) => `Revised by ${who}`,
    historyUnknown: "a removed user",
    sourcesHint:
      "Read-only databases mounted here. Mounting ingests the schema so Ask knows which tables exist before writing SQL.",
    mount: "Mount",
    unmount: "Unmount",
    syncSchema: "Refresh schema",
    schemaSynced: (n: number) => `Schema ingested (${n} tables)`,
    // Mounted, schema did not. **Do not call this a failed mount** — the source is mounted
    schemaFailed:
      "The data source is mounted, but its schema could not be ingested — Ask cannot see which tables exist. " +
      "This is in the alert centre; check the connection, then use Refresh schema.",
    explore: "Explore mappings",
    exploreHint:
      "An agent reads these schemas and proposes metric and dimension definitions. Proposals land in Pending; Ask uses them only once confirmed.",
    exploreQueued: "Exploration queued — proposals will appear under Pending.",
    sourcesEmpty: "No data sources mounted.",
    sourcesNoneAvailable:
      "No data sources registered yet — ask a deployment admin to register one.",
    newConn: "Register a new connection",
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
    railMappings: "Data mapping",
    railViolations: "Axioms",
    railDefects: "Ontology",
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
    defects: "Ontology contradicts itself",
    defectsHint:
      "Problems in the definitions themselves — no facts involved. These come first: while a definition contradicts itself, every fact-level finding that rests on it is suspect.",
    defectSymAsym: "Declared both symmetric and asymmetric",
    defectTransFunc: "Transitive and functional at once",
    defectCycle: "subClassOf runs in a circle",
    defectDisjointAncestor: "Disjoint with its own ancestor",
    defectInheritsDisjoint: "Inherits from two disjoint classes",
    defectInverseSelf: "Its own inverse — say symmetric instead",
    defectInverseNotMutual: "The inverse does not point back",
    defectSubPropertyCycle: "subPropertyOf runs in a circle",
    defectNeverInstantiable: "no instance can ever satisfy it",
    defectFixed: "I fixed the ontology",
    defectAccepted: "Leave it",
    runInference: "Run inference",
    inferring: "Inferring…",
    inferenceNoRules:
      "No transitive or symmetric property is declared, so there is no rule to run.",
    inferenceAdded: (n: number) => `${n} facts derived`,
    inferenceRetracted: (n: number) => `${n} retracted`,
    inferenceNothing: "Nothing new to derive",
    inferenceCapped: (n: number) =>
      `${n} predicate(s) hit the per-predicate limit and were not closed fully`,
    violations: "Axiom violations",
    violationsHint:
      "Facts that contradict axioms your ontology declares. Nothing here is a guess — a predicate that declares no axioms is never checked.",
    violationSelfLoop: "Points at itself",
    violationAsymmetry: "Both directions asserted",
    violationCycle: "Cycle through the transitive chain",
    violationFunctional: "Should hold one value, holds two",
    violationVia: (p: string) => `via ${p}`,
    violationPath: (n: number) => `${n} facts in the cycle`,
    retractFact: "Data is wrong",
    relaxAxiom: "Axiom is wrong",
    acceptBoth: "Both are right",
    runCheck: "Run check",
    checkNeverRun:
      "Not checked yet. Contradictions are found by asking your ontology, so a run here only reports what its axioms actually say.",
    checking: "Checking…",
    checkNoAxioms:
      "No axioms declared, so nothing could be checked. Import an ontology that declares them.",
    checkFound: (n: number) => `${n} new`,
    /** 算出来了，但都是已经在队列里或已被裁决过的——说「3 处矛盾」而列表只有
     *  一条会让人以为界面漏了东西 */
    checkNothingNew: "Nothing new",
    checkClean: (n: number) => `${n} facts checked, no contradictions`,
    mappings: "Data mapping",
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
    materialize: "Materialize inferences",
    materializeNote:
      "Write facts the ontology entails into the ledger — transitive chains and symmetric pairs. Off by default: a declaration can be wrong, and this one changes the graph. Derived facts are marked and can be taken back.",
    inferEvery: "Re-derive every",
    minutes: "minutes",
    lastInference: (when: string) => `last run ${when}`,
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
    auditAllActions: "All actions",
    auditSince: "From this date",
    auditUntil: "Up to this date",
    auditClear: "Clear filters",
    auditTotal: (n: number) => `${n} events`,
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
    deactivatedTitle: "Deactivated accounts",
    deactivatedHint:
      "They cannot sign in and do not appear in any member list. What they did is still attributed to them — that is why the account is kept rather than deleted.",
    reactivate: "Restore",
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
