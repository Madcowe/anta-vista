 📘 Distributed Search + Naming System (x0x-based) — Specification Summary
Anta-vita is a project to try and create a DNS and search engine analouge using x0x.

You should already have a x0x skill.md let me know if you can't find it.

The intitial aim is to build a rust libray for the api.

Below is a specfication made in discussion with another agent can you review...offer better ideas if you have and then create an implemenation plan.

Inital proof of concept is to make to initally make all apps based on i use MiniLM for embedding as should run onver very low spec..is this a good choice?

However we may want to develop it further to allow multiple models in later interations.


🧠 1. High-Level Goal
A decentralised, agent-based search and naming system built on x0x that:

    Works with no central index or authority
    Uses semantic search (embeddings) instead of keywords
    Builds trust and ranking emergently
    Supports heterogeneous file types
    Operates on low-spec machines (CPU-only)

🧩 2. Core Concepts
2.1 Entities
Agent

    Identified by cryptographic key
    Can:
        publish content
        index content
        respond to queries
        provide feedback

Resource (Page / File)
Represents anything searchable:
JSON
{
"id": "content-hash-or-uri",
"type": "text | image | audio | file",
"location": "uri_or_reference"
}
Show more lines
Claim
The fundamental unit of shared knowledge:
JSON
{
"subject": "resource_id",
"predicate": "about | tagged_as | useful_for | resolves_to",
"object": "value",
"by": "agent_id",
"timestamp": 1716980000,
"signature": "agent_signature"
}
Show more lines
🔍 3. Indexing Pipeline
3.1 File Ingestion
For each resource:
Step 1 — Detect MIME type

    Use content inspection (e.g. libmagic)
    Do NOT rely on filename extensions

Example:

image/jpeg
audio/mpeg
application/pdf

Step 2 — Extract metadata (if available)
Examples:

    Image → EXIF
    Audio → ID3 (title, artist)
    PDF → text content, title
    File path → directory names

Step 3 — Parse filename
Example:

cheesy.mp3 → "cheesy"

Step 4 — Generate semantic description (critical step)
Build natural language description:

"{filename terms} + {file type meaning} + {metadata}"

File type → semantic mapping
Example:
JSON
{
"image/jpeg": ["image", "picture", "photo"],
"audio/mpeg": ["audio", "music", "song"],
"application/pdf": ["document", "report"]
}

Show more lines
Example outputs

fish.jpg →
"a fish image file in jpeg format"

cheesy.mp3 →
"a cheesy music audio file"

unknown file (image/jpeg) →
"a photograph or image file"

3.2 Embedding Generation
Standard model (v1 requirement)
All agents use:

sentence-transformers/all-MiniLM-L6-v2

Properties:

    384 dimensions
    CPU-friendly
    small (~22M params)
    FOSS (Apache 2.0)

Embedding format
JSON
{
"resource": "resource_id",
"embedding": [...384 floats...],
"model": "all-MiniLM-L6-v2",
"normalized": true
}
Show more lines
Requirement

    consistent preprocessing
    same model version
    L2 normalisation

🔎 4. Query Processing
4.1 Query handling
Input:

"user natural language query"

Step 1
Generate embedding using same model
Step 2
Find candidate resources:

    local index
    network responses (via x0x gossip)

Step 3
Compute similarity:

    cosine similarity between vectors

4.2 Ranking function
Combine:

score =
  semantic_similarity
+ agreement_between_agents
+ weighted_feedback
+ trust_weight
+ recency (optional)

🤝 5. Trust Model
5.1 Initial state (cold start)

    All agents start with:

    trust = 0

    No authority assumptions

5.2 Trust emergence
Trust increases based on:

    agreement with other agents
    consistency over time
    alignment with user feedback

5.3 Trust signals
1. Agreement
Agents whose claims match consensus → trust ↑
2. Consistency
Stable answers over time → trust ↑
3. Feedback alignment
Agreeing with trusted users → trust ↑
4. Behaviour penalties

    spam → trust ↓
    inconsistency → trust ↓

5.4 Trust usage
Trust is a weight, NOT a filter

    all results considered
    trust adjusts influence

🧊 6. Cold Start Strategy
When no trust exists:

    Gather multiple responses
    Cluster identical/similar answers
    Select largest consistent cluster
    Assign early reputation

👍 7. Human Feedback
7.1 Explicit signals
Users can provide:

    👍 useful
    👎 not useful
    🚫 incorrect
    ⭐ high confidence

7.2 Implicit signals
Automatically captured:

    click
    dwell time
    scroll depth
    quick return

7.3 Feedback format
JSON
{
"query": "query text",
"resource": "resource_id",
"feedback": "useful",
"agent": "user_id"
}
Show more lines
7.4 Feedback usage

    aggregated per resource
    weighted by user trust
    feeds into ranking

📡 8. Network Behaviour (x0x)
8.1 Communication

    gossip-based distribution
    pub/sub queries
    peer-to-peer responses

8.2 Data shared
Agents exchange:

    embeddings
    claims
    feedback
    trust signals

8.3 Local-first model
Agents:

    maintain local index
    recompute embeddings if needed
    cache results

📂 9. File-Type Handling Strategy
9.1 Text-first approach (v1 constraint)

    NO multimodal models (no image embedding etc.)
    EVERYTHING reduced to text descriptions

9.2 Supported inference via text
System relies on:

    MIME type → modality
    filename → semantic hints
    metadata → enrichment

9.3 Limitations (accepted)

    cannot detect actual image content
    relies on naming + metadata
    improves over time via feedback + tagging

🛡️ 10. Anti-Abuse Measures

    rate limiting per agent
    identity cost (lightweight)
    diversity requirement (avoid sybil clusters)
    trust decay for unreliable agents

🧠 11. Key Design Principles

    Meaning over keywords
        embeddings replace keyword search
    Claims over documents
        knowledge is structured assertions
    Trust is emergent
        no central authority
    Local reproducibility
        agents can recompute embeddings
    Metadata → language → embedding
        all content expressed as text before embedding

🚀 12. Minimal Viable System (MVP)
To implement first:

    ✅ MIME detection
    ✅ filename parsing
    ✅ description generation
    ✅ MiniLM embeddings
    ✅ cosine similarity search
    ✅ simple feedback (like/dislike)
    ✅ basic clustering (for cold start)

🧠 Final Summary (for Claude)
This system is:

    A decentralised semantic search network where agents share embeddings, claims, and feedback, and where truth and ranking emerge from agreement, usage, and trust over time.
