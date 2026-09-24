"""
TapirusDB LlamaIndex Integration.
Provides TapirusVectorStore for LlamaIndex RAG pipelines.
"""

from typing import Any, Dict, List, Optional
from tapirus import Tapirus

try:
    from llama_index.core.vector_stores.types import (
        BasePydanticVectorStore,
        VectorStoreQuery,
        VectorStoreQueryResult,
        VectorStoreQueryMode,
    )
    from llama_index.core.schema import BaseNode, TextNode
except ImportError:
    # Graceful fallback if llama_index is not installed
    class BasePydanticVectorStore:  # type: ignore
        pass

    class VectorStoreQuery:  # type: ignore
        def __init__(self, query_embedding: Optional[List[float]] = None, similarity_top_k: int = 4):
            self.query_embedding = query_embedding
            self.similarity_top_k = similarity_top_k

    class VectorStoreQueryResult:  # type: ignore
        def __init__(self, nodes: List[Any], similarities: List[float], ids: List[str]):
            self.nodes = nodes
            self.similarities = similarities
            self.ids = ids

    class BaseNode:  # type: ignore
        pass

    class TextNode(BaseNode):  # type: ignore
        def __init__(self, text: str = "", id_: str = "", embedding: Optional[List[float]] = None):
            self.text = text
            self.id_ = id_
            self.embedding = embedding
            self.metadata: Dict[str, Any] = {}

        def get_content(self) -> str:
            return self.text


class TapirusLlamaVectorStore(BasePydanticVectorStore):
    """
    TapirusDB VectorStore integration for LlamaIndex.
    Embeds nodes and runs native in-engine HNSW similarity search on a local single-file database.
    """

    stores_text: bool = True
    is_embedding_query: bool = True

    def __init__(
        self,
        db: Optional[Tapirus] = None,
        path: Optional[str] = None,
        table_name: str = "llamaindex_vectors",
        dimension: int = 128,
        **kwargs: Any,
    ):
        super().__init__(**kwargs)
        self._db = db or (Tapirus.open(path) if path else Tapirus.open_in_memory())
        self._table_name = table_name
        self._dimension = dimension
        self._init_table()

    def _init_table(self):
        sql = f"""
        CREATE TABLE IF NOT EXISTS {self._table_name} (
            id INTEGER PRIMARY KEY,
            node_id TEXT,
            text TEXT,
            metadata TEXT,
            embedding VECTOR({self._dimension})
        );
        """
        self._db.execute(sql)

    @property
    def client(self) -> Any:
        return self._db

    def add(self, nodes: List[BaseNode], **add_kwargs: Any) -> List[str]:
        """Add nodes to the vector store."""
        import json

        ids = []
        for i, node in enumerate(nodes):
            text = node.get_content() if hasattr(node, "get_content") else getattr(node, "text", "")
            node_id = getattr(node, "node_id", getattr(node, "id_", str(i)))
            meta = getattr(node, "metadata", {})
            meta_json = json.dumps(meta)

            emb = getattr(node, "embedding", None) or [0.0] * self._dimension
            emb_str = "[" + ",".join(f"{x:.6f}" for x in emb) + "]"

            escaped_text = text.replace("'", "''")
            escaped_meta = meta_json.replace("'", "''")
            escaped_node_id = str(node_id).replace("'", "''")

            sql = f"""
            INSERT INTO {self._table_name} (node_id, text, metadata, embedding)
            VALUES ('{escaped_node_id}', '{escaped_text}', '{escaped_meta}', {emb_str});
            """
            self._db.execute(sql)
            ids.append(str(node_id))
        return ids

    def query(self, query: VectorStoreQuery, **kwargs: Any) -> VectorStoreQueryResult:
        """Query index for top_k most similar nodes."""
        import json

        embedding = query.query_embedding or [0.0] * self._dimension
        k = query.similarity_top_k or 4

        emb_str = "[" + ",".join(f"{x:.6f}" for x in embedding) + "]"
        sql = f"""
        SELECT node_id, text, metadata, VECTOR_DISTANCE(embedding, {emb_str}) AS dist
        FROM {self._table_name}
        ORDER BY dist ASC
        LIMIT {k};
        """
        rows = self._db.query(sql)

        nodes: List[TextNode] = []
        similarities: List[float] = []
        ids: List[str] = []

        for r in rows:
            node_id = str(r.get("node_id", ""))
            text = str(r.get("text", ""))
            meta_str = r.get("metadata", "{}")
            try:
                meta = json.loads(meta_str) if isinstance(meta_str, str) else meta_str
            except Exception:
                meta = {}

            node = TextNode(text=text, id_=node_id)
            node.metadata = meta
            nodes.append(node)

            dist = float(r.get("dist", 1.0))
            # Convert cosine distance to similarity score
            sim = 1.0 - dist
            similarities.append(sim)
            ids.append(node_id)

        return VectorStoreQueryResult(nodes=nodes, similarities=similarities, ids=ids)

    def delete(self, ref_doc_id: str, **delete_kwargs: Any) -> None:
        """Delete nodes with given reference document ID."""
        escaped_id = ref_doc_id.replace("'", "''")
        sql = f"DELETE FROM {self._table_name} WHERE node_id = '{escaped_id}';"
        self._db.execute(sql)
