# sift weight sweep (Julia)
# Requires Julia + a labeled corpus in docs/labels.json.
# Strategy: coarse grid over rrf_k, recency weight, snippet_tokens; for each
# candidate write weights.toml, run scripts/measure.py, keep best recall@5.

using JSON

labels = JSON.parsefile("docs/labels.json")
best = nothing
for rrf_k in [30, 60, 90]
    for w_rec in [0.5, 1.0, 1.5]
        for snippet_tokens in [12, 18, 24]
            open("weights.toml", "w") do io
                println(io, "rrf_k = $rrf_k")
                println(io, "w_rec = $w_rec")
                println(io, "snippet_tokens = $snippet_tokens")
            end
            run(`python3 scripts/measure.py`)
            # parse recall line manually in a real implementation
        end
    end
end
println("sweep finished; inspect results")
EOF