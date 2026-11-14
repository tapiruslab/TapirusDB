--------------------------- MODULE TAPIRUS_WAL ---------------------------
(*
 * TapirusDB Formal Crash-Safety & WAL Recovery Specification
 *
 * Models TapirusDB's Single-Writer page-level Write-Ahead Logging (WAL)
 * protocol, verifying ACID Atomicity, Durability, and Crash Recovery
 * invariants under arbitrary crash and checkpoint interleavings.
 *)

EXTENDS Integers, Sequences, FiniteSets

CONSTANTS
    MaxPages,       \* Number of database pages (e.g. 2)
    MaxTxId,        \* Maximum Transaction ID (e.g. 2)
    MaxVal,         \* Maximum value domain (e.g. 2)
    MaxWalLen,      \* Bounded WAL log length for finite model checking (e.g. 4)
    Nil             \* Null/Empty marker

VARIABLES
    diskPages,          \* Physical on-disk pages: [PageId -> 0..MaxVal]
    bufferCache,        \* In-memory dirty pages: [PageId -> (1..MaxVal) \cup {Nil}]
    walLog,             \* Sequence of WAL records: << [tx, page, val, is_commit] >>
    lastCheckpointLsn,  \* Last durable checkpoint LSN stored in WAL header
    txStatus,           \* Status of transactions: [TxId -> {"Active", "Committed", "Aborted"}]
    activeWriter,       \* Single-Writer lock holder: TxId \cup {Nil}
    expectedDisk,       \* Reference oracle of committed disk image: [PageId -> 0..MaxVal]
    systemState         \* System state: {"Running", "Crashed", "Recovered"}

vars == << diskPages, bufferCache, walLog, lastCheckpointLsn, txStatus, activeWriter, expectedDisk, systemState >>

Pages == 1..MaxPages
TxIds == 1..MaxTxId
Values == 1..MaxVal

TypeOK ==
    /\ diskPages \in [Pages -> 0..MaxVal]
    /\ bufferCache \in [Pages -> Values \cup {Nil}]
    /\ lastCheckpointLsn \in 0..Len(walLog)
    /\ Len(walLog) <= MaxWalLen
    /\ txStatus \in [TxIds -> {"Active", "Committed", "Aborted"}]
    /\ activeWriter \in TxIds \cup {Nil}
    /\ expectedDisk \in [Pages -> 0..MaxVal]
    /\ systemState \in {"Running", "Crashed", "Recovered"}

(* ------------------- INITIAL STATE ------------------- *)

Init ==
    /\ diskPages = [p \in Pages |-> 0]
    /\ bufferCache = [p \in Pages |-> Nil]
    /\ walLog = << >>
    /\ lastCheckpointLsn = 0
    /\ txStatus = [t \in TxIds |-> "Active"]
    /\ activeWriter = Nil
    /\ expectedDisk = [p \in Pages |-> 0]
    /\ systemState = "Running"

(* ------------------- HELPER OPERATORS ------------------- *)

\* Latest committed WAL value for page `p` across frame indices `frameSet`
HasFrameForPage(frameSet, p) ==
    \E i \in frameSet : walLog[i].page = p

LastFrameVal(frameSet, p) ==
    walLog[CHOOSE i \in frameSet :
        walLog[i].page = p /\
        (\A j \in frameSet : walLog[j].page = p => j <= i)].val

(* ------------------- SYSTEM ACTIONS ------------------- *)

\* Acquire the exclusive Single-Writer lock for an active transaction
BeginTx(tx) ==
    /\ systemState = "Running"
    /\ activeWriter = Nil
    /\ txStatus[tx] = "Active"
    /\ activeWriter' = tx
    /\ UNCHANGED << diskPages, bufferCache, walLog, lastCheckpointLsn, txStatus, expectedDisk, systemState >>

\* Active writer modifies a page in the in-memory buffer cache
WriteBuffer(tx, page, val) ==
    /\ systemState = "Running"
    /\ activeWriter = tx
    /\ txStatus[tx] = "Active"
    /\ bufferCache' = [bufferCache EXCEPT ![page] = val]
    /\ UNCHANGED << diskPages, walLog, lastCheckpointLsn, txStatus, activeWriter, expectedDisk, systemState >>

\* Append a modified page frame to the Write-Ahead Log
AppendWalFrame(tx, page, val) ==
    /\ systemState = "Running"
    /\ activeWriter = tx
    /\ txStatus[tx] = "Active"
    /\ Len(walLog) < MaxWalLen
    /\ walLog' = Append(walLog, [tx |-> tx, page |-> page, val |-> val, is_commit |-> FALSE])
    /\ UNCHANGED << diskPages, bufferCache, lastCheckpointLsn, txStatus, activeWriter, expectedDisk, systemState >>

\* Commit transaction: append commit marker, publish durable state, release writer lock
CommitTx(tx) ==
    /\ systemState = "Running"
    /\ activeWriter = tx
    /\ txStatus[tx] = "Active"
    /\ Len(walLog) < MaxWalLen
    /\ LET txFrames == {i \in (lastCheckpointLsn + 1)..Len(walLog) :
                            walLog[i].tx = tx /\ ~walLog[i].is_commit}
       IN expectedDisk' = [p \in Pages |->
            IF HasFrameForPage(txFrames, p)
            THEN LastFrameVal(txFrames, p)
            ELSE expectedDisk[p]]
    /\ walLog' = Append(walLog, [tx |-> tx, page |-> 0, val |-> 0, is_commit |-> TRUE])
    /\ txStatus' = [txStatus EXCEPT ![tx] = "Committed"]
    /\ activeWriter' = Nil
    /\ UNCHANGED << diskPages, bufferCache, lastCheckpointLsn, systemState >>

\* Explicitly abort active writer transaction and discard dirty cache
AbortTx(tx) ==
    /\ systemState = "Running"
    /\ activeWriter = tx
    /\ txStatus[tx] = "Active"
    /\ txStatus' = [txStatus EXCEPT ![tx] = "Aborted"]
    /\ bufferCache' = [p \in Pages |-> Nil]
    /\ activeWriter' = Nil
    /\ UNCHANGED << diskPages, walLog, lastCheckpointLsn, expectedDisk, systemState >>

\* Checkpoint: Flush committed WAL frames when no uncommitted writer holds an open transaction
Checkpoint ==
    /\ systemState = "Running"
    /\ activeWriter = Nil
    /\ Len(walLog) > lastCheckpointLsn
    /\ LET committedFrames == {i \in (lastCheckpointLsn + 1)..Len(walLog) :
                                /\ ~walLog[i].is_commit
                                /\ txStatus[walLog[i].tx] = "Committed"}
       IN diskPages' = [p \in Pages |->
            IF HasFrameForPage(committedFrames, p)
            THEN LastFrameVal(committedFrames, p)
            ELSE diskPages[p]]
    /\ lastCheckpointLsn' = Len(walLog)
    /\ bufferCache' = [p \in Pages |-> Nil]
    /\ UNCHANGED << walLog, txStatus, activeWriter, expectedDisk, systemState >>

\* Arbitrary system crash (power loss / process termination)
Crash ==
    /\ systemState = "Running"
    /\ systemState' = "Crashed"
    /\ bufferCache' = [p \in Pages |-> Nil]
    /\ activeWriter' = Nil
    /\ UNCHANGED << diskPages, walLog, lastCheckpointLsn, txStatus, expectedDisk >>

\* Crash recovery: Replay committed WAL frames from lastCheckpointLsn forward
Recover ==
    /\ systemState = "Crashed"
    /\ LET committedTxs == {walLog[i].tx :
                                i \in {k \in (lastCheckpointLsn + 1)..Len(walLog) : walLog[k].is_commit}}
           replayFrames == {i \in (lastCheckpointLsn + 1)..Len(walLog) :
                                walLog[i].tx \in committedTxs /\ ~walLog[i].is_commit}
       IN diskPages' = [p \in Pages |->
            IF HasFrameForPage(replayFrames, p)
            THEN LastFrameVal(replayFrames, p)
            ELSE diskPages[p]]
    /\ systemState' = "Recovered"
    /\ txStatus' = [t \in TxIds |-> IF txStatus[t] = "Committed" THEN "Committed" ELSE "Aborted"]
    /\ UNCHANGED << bufferCache, walLog, lastCheckpointLsn, activeWriter, expectedDisk >>

\* Stuttering step after terminal recovery state
Done ==
    /\ systemState = "Recovered"
    /\ UNCHANGED vars

(* ------------------- STATE TRANSITIONS ------------------- *)

Next ==
    \/ \E tx \in TxIds : BeginTx(tx)
    \/ \E tx \in TxIds, p \in Pages, v \in Values : WriteBuffer(tx, p, v)
    \/ \E tx \in TxIds, p \in Pages, v \in Values : AppendWalFrame(tx, p, v)
    \/ \E tx \in TxIds : CommitTx(tx)
    \/ \E tx \in TxIds : AbortTx(tx)
    \/ Checkpoint
    \/ Crash
    \/ Recover
    \/ Done

Spec == Init /\ [][Next]_vars

(* ------------------- SAFETY INVARIANTS ------------------- *)

\* After recovery, physical disk matches exact committed oracle state:
\* neither committed writes are lost nor uncommitted/aborted writes leak to disk.
RecoveredDiskMatchesCommitted ==
    systemState = "Recovered" => diskPages = expectedDisk

\* When idle and fully checkpointed (WAL drained), physical disk also equals committed oracle
CheckpointedDiskMatchesCommitted ==
    (systemState = "Running" /\ activeWriter = Nil /\ lastCheckpointLsn = Len(walLog)) =>
        diskPages = expectedDisk

\* Active transactions never have uncommitted buffer values flushed onto disk
NoUncommittedCheckpoint ==
    \A t \in TxIds :
        (txStatus[t] = "Active" /\ activeWriter = t) =>
            (\A i \in 1..lastCheckpointLsn : walLog[i].tx # t)

=============================================================================
