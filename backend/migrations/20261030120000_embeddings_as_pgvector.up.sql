-- Store embeddings as pgvector vectors and search them in the database.
CREATE EXTENSION IF NOT EXISTS vector;

-- Decode one big-endian IEEE 754 single from the bytes written by float4send.
-- Stored embeddings are normalized, so infinities and NaN do not occur.
CREATE FUNCTION pg_temp.float4_from_bytes(bytes BYTEA) RETURNS REAL
LANGUAGE sql IMMUTABLE AS $$
    SELECT (
        CASE WHEN bits < 0 THEN -1 ELSE 1 END
        * CASE
            WHEN exponent = 0 THEN mantissa * 2::FLOAT8 ^ -149
            ELSE (8388608 + mantissa) * 2::FLOAT8 ^ (exponent - 150)
          END
    )::REAL
    FROM (
        SELECT bits, (bits >> 23) & 255 AS exponent, bits & 8388607 AS mantissa
        FROM (SELECT ('x' || encode(bytes, 'hex'))::BIT(32)::INT4 AS bits) b
    ) parts
$$;

DELETE FROM feed_item_embeddings WHERE length(embedding) < 4;

ALTER TABLE feed_item_embeddings ADD COLUMN embedding_vector vector;

UPDATE feed_item_embeddings e
SET embedding_vector = (
    SELECT array_agg(pg_temp.float4_from_bytes(substring(e.embedding FROM n FOR 4)) ORDER BY n)
    FROM generate_series(1, length(e.embedding) - 3, 4) AS n
)::vector;

ALTER TABLE feed_item_embeddings DROP COLUMN embedding;
ALTER TABLE feed_item_embeddings RENAME COLUMN embedding_vector TO embedding;
ALTER TABLE feed_item_embeddings ALTER COLUMN embedding SET NOT NULL;

-- Only the in-memory index read embeddings by insertion time.
DROP INDEX IF EXISTS idx_feed_item_embeddings_model_embedded_at;

-- The column has no fixed dimension, so models of any size can be stored.
-- HNSW needs one, so each model and dimension gets its own partial index on
-- a typed cast. Index names are derived from both, and creation is skipped
-- above the 2000 dimensions HNSW supports (searches then scan exactly).
CREATE FUNCTION feed_item_embedding_index_name(p_model TEXT, p_dims INT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT 'feed_item_embeddings_hnsw_' || left(md5(p_model || ':' || p_dims), 16)
$$;

CREATE FUNCTION ensure_feed_item_embedding_index(p_model TEXT, p_dims INT) RETURNS VOID
LANGUAGE plpgsql AS $$
DECLARE
    index_name TEXT := feed_item_embedding_index_name(p_model, p_dims);
BEGIN
    IF p_dims < 1 OR p_dims > 2000 OR to_regclass(index_name) IS NOT NULL THEN
        RETURN;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtext(index_name));
    IF to_regclass(index_name) IS NOT NULL THEN
        RETURN;
    END IF;
    EXECUTE format(
        'CREATE INDEX %I ON feed_item_embeddings
         USING hnsw ((embedding::vector(%s)) vector_ip_ops)
         WHERE model = %L AND vector_dims(embedding) = %s',
        index_name, p_dims, p_model, p_dims
    );
END
$$;

-- Collect first: an index cannot be created while a query reads the table.
DO $$
DECLARE
    pairs TEXT[][];
    pair TEXT[];
BEGIN
    SELECT array_agg(ARRAY[model, dims::TEXT])
    INTO pairs
    FROM (SELECT DISTINCT model, vector_dims(embedding) AS dims FROM feed_item_embeddings) e;
    IF pairs IS NULL THEN
        RETURN;
    END IF;
    FOREACH pair SLICE 1 IN ARRAY pairs
    LOOP
        PERFORM ensure_feed_item_embedding_index(pair[1], pair[2]::INT);
    END LOOP;
END
$$;

-- The `p_limit` nearest items to `p_query` (approximately, via the HNSW index),
-- plus the items in `p_include`, each with its cosine similarity and ranking
-- date. Embeddings are normalized, so the inner product is the cosine.
--
-- The statement is built per call so the model and dimension are literals
-- that match an index's cast and predicate.
CREATE FUNCTION nearest_feed_item_embeddings(
    p_model TEXT,
    p_query vector,
    p_feed_id BIGINT,
    p_limit INT,
    p_include BIGINT[]
)
RETURNS TABLE (id BIGINT, similarity REAL, date TIMESTAMPTZ)
LANGUAGE plpgsql AS $$
DECLARE
    dims INT := vector_dims(p_query);
BEGIN
    PERFORM set_config('hnsw.ef_search', LEAST(GREATEST(p_limit, 40), 1000)::TEXT, true);
    -- Keep scanning when the feed filter rejects candidates.
    PERFORM set_config('hnsw.iterative_scan', 'relaxed_order', true);
    RETURN QUERY EXECUTE format(
        $query$
        WITH nearest AS (
            SELECT e.feed_item_id, e.embedding::vector(%1$s) <#> $1::vector(%1$s) AS distance
            FROM feed_item_embeddings e
            INNER JOIN feed_items f ON f.id = e.feed_item_id
            WHERE e.model = %2$L AND vector_dims(e.embedding) = %1$s
              AND ($2::BIGINT IS NULL OR f.feed_id = $2)
            ORDER BY distance
            LIMIT $3
        ),
        included AS (
            SELECT e.feed_item_id, e.embedding::vector(%1$s) <#> $1::vector(%1$s) AS distance
            FROM feed_item_embeddings e
            WHERE e.model = %2$L AND vector_dims(e.embedding) = %1$s
              AND e.feed_item_id = ANY($4)
        ),
        candidates AS (
            SELECT * FROM nearest
            UNION
            SELECT * FROM included
        )
        SELECT c.feed_item_id, (-c.distance)::REAL, LEAST(d.published_at, f.inserted_at)
        FROM candidates c
        INNER JOIN feed_items f ON f.id = c.feed_item_id
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        $query$,
        dims, p_model
    ) USING p_query, p_feed_id, p_limit, COALESCE(p_include, '{}');
END
$$;
