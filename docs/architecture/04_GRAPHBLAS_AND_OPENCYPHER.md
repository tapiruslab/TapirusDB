# Graph Storage and Query Support: Implementation Notes

> Status: implementation overview. The query syntax is a TapirusDB subset; it is not a claim of full openCypher or GraphBLAS conformance.

## Graph representation

`GraphEngine` stores graph data in its runtime structures and provides operations for adding nodes and edges, finding neighbors and paths, extracting subgraphs, and computing selected graph algorithms. `to_csr()` converts a graph to a compressed sparse row representation for code that needs contiguous adjacency arrays.

The conversion API does not mean all graph queries run over a persistent CSR index. Do not infer allocation behavior, cache residency, or traversal latency from the CSR data shape. Those depend on graph size and the operation being run.

## Query syntax

The SQL parser accepts a limited `GRAPH MATCH` pattern syntax, and the Cypher module handles a bounded `MATCH`/`CREATE` subset. This is not full openCypher. Check the parser and executor for supported patterns, filters, projections, and mutation behavior before using a query in an application.

## Algorithms

The SQL executor exposes selected graph algorithms, including PageRank, connected components, betweenness, and Louvain. Availability of a name is not a correctness or scale guarantee. Validate results against a known graph and workload before using them for consequential decisions.

## Measurement

No traversal performance figure is promised here. Measure the complete query on representative data, including graph construction or CSR conversion if that is part of the application path.
