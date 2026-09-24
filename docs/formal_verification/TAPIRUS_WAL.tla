--------------------------- MODULE TAPIRUS_WAL ---------------------------
(*
 * TapirusDB Formal Crash-Safety & WAL Recovery Specification
 * Architected by Ahmad Faiz • Tapirus Tech Lab (TapirusDB.com)
 *
 * Mathematically models the page-level Write-Ahead Logging (WAL) protocol,
 * ensuring ACID Atomicity, Durability, and Crash Recovery Invariance.
 *)

EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS 
    MaxPages,       \* Number of database pages (e.g., 1..3)
    MaxTxId,        \* Maximum Transaction ID
    Nil             \* Null/Empty marker

VARIABLES
    diskPages,          \* Physical on-disk pages: [PageId -> Value]
    bufferCache,        \* In-memory dirty pages: [PageId -> Value]
    walLog,             \* Sequence of WAL records: << [tx, page, val, is_commit] >>
    lastCheckpointLsn,  \* Last durable checkpoint LSN stored in WAL header
    txStatus,           \* Status of transactions: [TxId -> {"Active", "Committed", "Aborted"}]
    systemState         \* System state: "Running", "Crashed", "Recovered"

vars == << diskPages, bufferCache, walLog, lastCheckpointLsn, txStatus, systemState >>

Pages == 1..MaxPages
TxIds == 1..MaxTxId

TypeOK ==
    /\ diskPages \in [Pages -> Integers]
    /\ bufferCache \in [Pages -> Integers \cup {Nil}]
    /\ lastCheckpointLsn \in 0..Len(walLog)
    /\ txStatus \in [TxIds -> {"Active", "Committed", "Aborted"}]
    /\ systemState \in {"Running", "Crashed", "Recovered"}

(* ------------------- INITIAL STATE ------------------- *)

Init ==
    /\ diskPages = [p \in Pages |-> 0]
    /\ bufferCache = [p \in Pages |-> Nil]
    /\ walLog = << >>
    /\ lastCheckpointLsn = 0
    /\ txStatus = [t \in TxIds |-> "Active"]
    /\ systemState = "Running"

(* ------------------- SYSTEM ACTIONS ------------------- *)

\* Transaction modifies a page in memory buffer
WriteBuffer(tx, page, val) ==
    /\ systemState = "Running"
    /\ txStatus[tx] = "Active"
    /\ bufferCache' = [bufferCache EXCEPT ![page] = val]
    /\ UNCHANGED << diskPages, walLog, lastCheckpointLsn, txStatus, systemState >>

\* Append modified page frame to Write-Ahead Log before evicting
AppendWalFrame(tx, page, val) ==
    /\ systemState = "Running"
    /\ txStatus[tx] = "Active"
    /\ walLog' = Append(walLog, [tx |-> tx, page |-> page, val |-> val, is_commit |-> FALSE])
    /\ UNCHANGED << diskPages, bufferCache, lastCheckpointLsn, txStatus, systemState >>

\* Commit transaction by writing a commit frame and persisting to WAL
CommitTx(tx) ==
    /\ systemState = "Running"
    /\ txStatus[tx] = "Active"
    /\ walLog' = Append(walLog, [tx |-> tx, page |-> 0, val |-> 0, is_commit |-> TRUE])
    /\ txStatus' = [txStatus EXCEPT ![tx] = "Committed"]
    /\ UNCHANGED << diskPages, bufferCache, lastCheckpointLsn, systemState >>

\* Checkpoint: Flush all committed pages from WAL up to current log length into main .tapir disk file
Checkpoint ==
    /\ systemState = "Running"
    /\ Len(walLog) > lastCheckpointLsn
    /\ LET committedFrames == {i \in (lastCheckpointLsn + 1)..Len(walLog) : 
                                /\ ~walLog[i].is_commit 
                                /\ txStatus[walLog[i].tx] = "Committed"}
       IN diskPages' = [p \in Pages |-> 
            IF \E i \in committedFrames : walLog[i].page = p
            THEN walLog[CHOOSE i \in committedFrames : 
                    walLog[i].page = p /\ 
                    (\A j \in committedFrames : walLog[j].page = p => j <= i)].val
            ELSE diskPages[p]
          ]
    /\ lastCheckpointLsn' = Len(walLog)
    /\ bufferCache' = [p \in Pages |-> Nil]
    /\ UNCHANGED << walLog, txStatus, systemState >>

\* Arbitrary system crash (power failure / panic)
Crash ==
    /\ systemState = "Running"
    /\ systemState' = "Crashed"
    /\ bufferCache' = [p \in Pages |-> Nil]
    /\ UNCHANGED << diskPages, walLog, lastCheckpointLsn, txStatus >>

\* Crash recovery: Replay committed WAL frames from lastCheckpointLsn forward
Recover ==
    /\ systemState = "Crashed"
    /\ LET committedTxs == {walLog[i].tx : i \in (lastCheckpointLsn + 1)..Len(walLog) 
                            WHERE walLog[i].is_commit}
           replayFrames == {i \in (lastCheckpointLsn + 1)..Len(walLog) : 
                            walLog[i].tx \in committedTxs /\ ~walLog[i].is_commit}
       IN diskPages' = [p \in Pages |-> 
            IF \E i \in replayFrames : walLog[i].page = p
            THEN walLog[CHOOSE i \in replayFrames : 
                    walLog[i].page = p /\ 
                    (\A j \in replayFrames : walLog[j].page = p => j <= i)].val
            ELSE diskPages[p]
          ]
    /\ systemState' = "Recovered"
    /\ txStatus' = [t \in TxIds |-> IF txStatus[t] = "Committed" THEN "Committed" ELSE "Aborted"]
    /\ UNCHANGED << bufferCache, walLog, lastCheckpointLsn >>

(* ------------------- STATE TRANSITIONS ------------------- *)

Next ==
    \/ \E tx \in TxIds, p \in Pages, v \in 1..10 : WriteBuffer(tx, p, v)
    \/ \E tx \in TxIds, p \in Pages, v \in 1..10 : AppendWalFrame(tx, p, v)
    \/ \E tx \in TxIds : CommitTx(tx)
    \/ Checkpoint
    \/ Crash
    \/ Recover

Spec == Init /\ [][Next]_vars

(* ------------------- SAFETY INVARIANTS ------------------- *)

\* Invariant: An uncommitted transaction's dirty buffer values NEVER persist on disk after recovery
NoUncommittedDataPersisted ==
    systemState = "Recovered" =>
        \A tx \in TxIds : txStatus[tx] = "Aborted" =>
            \A i \in 1..Len(walLog) :
                (walLog[i].tx = tx /\ ~walLog[i].is_commit) =>
                    diskPages[walLog[i].page] /= walLog[i].val

\* Invariant: All committed transactions are durably preserved after recovery
CommittedDataDurability ==
    systemState = "Recovered" =>
        \A tx \in TxIds : txStatus[tx] = "Committed" =>
            \A i \in 1..Len(walLog) :
                (walLog[i].tx = tx /\ ~walLog[i].is_commit /\
                 (\A j \in (i + 1)..Len(walLog) : walLog[j].page = walLog[i].page => walLog[j].tx /= tx)) =>
                    diskPages[walLog[i].page] = walLog[i].val

=============================================================================
