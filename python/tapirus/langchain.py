"""
TapirusDB LangChain Integration.
Provides TapirusVectorStore and TapirusChatMessageHistory for seamless use in LangChain pipelines.
"""

from typing import Any, Callable, Dict, Iterable, List, Optional, Tuple
from tapirus import Tapirus

try:
    from langchain_core.vectorstores import VectorStore
    from langchain_core.documents import Document
    from langchain_core.embeddings import Embeddings
    from langchain_core.chat_history import BaseChatMessageHistory
    from langchain_core.messages import BaseMessage, HumanMessage, AIMessage, messages_from_dict, messages_to_dict
except ImportError:
    # Graceful fallback if langchain_core is not installed
    class VectorStore:  # type: ignore
        pass

    class Document:  # type: ignore
        def __init__(self, page_content: str, metadata: Optional[Dict[str, Any]] = None):
            self.page_content = page_content
            self.metadata = metadata or {}

    class Embeddings:  # type: ignore
        pass

    class BaseChatMessageHistory:  # type: ignore
        pass

    class BaseMessage:  # type: ignore
        def __init__(self, content: str):
            self.content = content

    class HumanMessage(BaseMessage):  # type: ignore
        pass

    class AIMessage(BaseMessage):  # type: ignore
        pass


class TapirusVectorStore(VectorStore):
    """
    TapirusDB VectorStore implementation for LangChain.
    Stores dense vectors and documents in an embedded single-file .tapir database.
    """

    def __init__(
        self,
        db: Optional[Tapirus] = None,
        path: Optional[str] = None,
        table_name: str = "langchain_vectors",
        embedding_function: Optional[Any] = None,
        dimension: int = 128,
    ):
        self.db = db or (Tapirus.open(path) if path else Tapirus.open_in_memory())
        self.table_name = table_name
        self.embedding_function = embedding_function
        self.dimension = dimension
        self._init_table()

    def _init_table(self):
        sql = f"""
        CREATE TABLE IF NOT EXISTS {self.table_name} (
            id INTEGER PRIMARY KEY,
            content TEXT,
            metadata TEXT,
            embedding VECTOR({self.dimension})
        );
        """
        self.db.execute(sql)

    def add_texts(
        self,
        texts: Iterable[str],
        metadatas: Optional[List[Dict[str, Any]]] = None,
        **kwargs: Any,
    ) -> List[str]:
        """Add texts to the vectorstore with embeddings."""
        import json

        texts_list = list(texts)
        if not texts_list:
            return []

        embeddings = []
        if self.embedding_function:
            if hasattr(self.embedding_function, "embed_documents"):
                embeddings = self.embedding_function.embed_documents(texts_list)
            elif callable(self.embedding_function):
                embeddings = [self.embedding_function(t) for t in texts_list]
        else:
            # Fallback zero-vector if no embedding function provided
            embeddings = [[0.0] * self.dimension for _ in texts_list]

        ids = []
        for i, text in enumerate(texts_list):
            meta = metadatas[i] if metadatas and i < len(metadatas) else {}
            meta_json = json.dumps(meta)
            emb = embeddings[i] if i < len(embeddings) else [0.0] * self.dimension
            emb_str = "[" + ",".join(f"{x:.6f}" for x in emb) + "]"

            escaped_text = text.replace("'", "''")
            escaped_meta = meta_json.replace("'", "''")

            sql = f"""
            INSERT INTO {self.table_name} (content, metadata, embedding)
            VALUES ('{escaped_text}', '{escaped_meta}', {emb_str});
            """
            self.db.execute(sql)
            ids.append(str(i))

        return ids

    def similarity_search(
        self,
        query: str,
        k: int = 4,
        **kwargs: Any,
    ) -> List[Document]:
        """Return docs most similar to query."""
        if self.embedding_function:
            if hasattr(self.embedding_function, "embed_query"):
                query_vector = self.embedding_function.embed_query(query)
            else:
                query_vector = self.embedding_function(query)
            return self.similarity_search_by_vector(query_vector, k=k, **kwargs)

        # Fallback to lexical query if no embedding function
        escaped = query.replace("'", "''")
        sql = f"SELECT content, metadata FROM {self.table_name} WHERE content LIKE '%{escaped}%' LIMIT {k};"
        rows = self.db.query(sql)
        import json

        docs = []
        for r in rows:
            meta = {}
            if r.get("metadata"):
                try:
                    meta = json.loads(r["metadata"])
                except Exception:
                    pass
            docs.append(Document(page_content=r.get("content", ""), metadata=meta))
        return docs

    def similarity_search_by_vector(
        self,
        embedding: List[float],
        k: int = 4,
        **kwargs: Any,
    ) -> List[Document]:
        """Return docs most similar to embedding vector using TapirusDB native vector search."""
        import json

        emb_str = "[" + ",".join(f"{x:.6f}" for x in embedding) + "]"
        sql = f"""
        SELECT content, metadata, VECTOR_DISTANCE(embedding, {emb_str}) AS dist
        FROM {self.table_name}
        ORDER BY dist ASC
        LIMIT {k};
        """
        rows = self.db.query(sql)
        docs = []
        for r in rows:
            meta = {}
            if r.get("metadata"):
                try:
                    meta = json.loads(r["metadata"])
                except Exception:
                    pass
            meta["distance"] = r.get("dist")
            docs.append(Document(page_content=r.get("content", ""), metadata=meta))
        return docs

    @classmethod
    def from_texts(
        cls,
        texts: List[str],
        embedding: Any,
        metadatas: Optional[List[Dict[str, Any]]] = None,
        path: Optional[str] = None,
        table_name: str = "langchain_vectors",
        dimension: int = 128,
        **kwargs: Any,
    ) -> "TapirusVectorStore":
        """Construct TapirusVectorStore from a list of texts."""
        store = cls(
            path=path,
            table_name=table_name,
            embedding_function=embedding,
            dimension=dimension,
        )
        store.add_texts(texts, metadatas=metadatas, **kwargs)
        return store


class TapirusChatMessageHistory(BaseChatMessageHistory):
    """
    Chat message history stored in an embedded TapirusDB database.
    Persists user and AI messages with session scoping.
    """

    def __init__(
        self,
        session_id: str,
        db: Optional[Tapirus] = None,
        path: Optional[str] = None,
        table_name: str = "tapirus_chat_history",
    ):
        self.session_id = session_id
        self.db = db or (Tapirus.open(path) if path else Tapirus.open_in_memory())
        self.table_name = table_name
        self._init_table()

    def _init_table(self):
        sql = f"""
        CREATE TABLE IF NOT EXISTS {self.table_name} (
            id INTEGER PRIMARY KEY,
            session_id TEXT,
            role TEXT,
            content TEXT,
            created_at INTEGER
        );
        """
        self.db.execute(sql)

    @property
    def messages(self) -> List[BaseMessage]:
        """Retrieve all messages for the current session."""
        escaped_session = self.session_id.replace("'", "''")
        sql = f"""
        SELECT role, content FROM {self.table_name}
        WHERE session_id = '{escaped_session}'
        ORDER BY id ASC;
        """
        rows = self.db.query(sql)
        msgs: List[BaseMessage] = []
        for r in rows:
            role = r.get("role", "")
            content = r.get("content", "")
            if role == "human":
                msgs.append(HumanMessage(content=content))
            elif role == "ai":
                msgs.append(AIMessage(content=content))
            else:
                msgs.append(BaseMessage(content=content))
        return msgs

    def add_message(self, message: BaseMessage) -> None:
        """Append a message to the session history."""
        import time

        role = "human" if isinstance(message, HumanMessage) else "ai"
        escaped_session = self.session_id.replace("'", "''")
        escaped_content = str(message.content).replace("'", "''")
        ts = int(time.time())

        sql = f"""
        INSERT INTO {self.table_name} (session_id, role, content, created_at)
        VALUES ('{escaped_session}', '{role}', '{escaped_content}', {ts});
        """
        self.db.execute(sql)

    def clear(self) -> None:
        """Clear all messages for the current session."""
        escaped_session = self.session_id.replace("'", "''")
        sql = f"DELETE FROM {self.table_name} WHERE session_id = '{escaped_session}';"
        self.db.execute(sql)
