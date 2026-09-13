---------------------------- MODULE FM1_Escrow ----------------------------
(* FM-1 — Escrow transfer, authority and rights conservation (SPEC-006 §8, §11, §16).   *)
(* This module states the same machine as crates/carolina-models/src/fm1.rs. The Rust   *)
(* explicit-state checker is the executed artifact; a TLC run of this module must be     *)
(* recorded separately (tool version, config, state counts) before it counts as evidence.*)

EXTENDS Naturals, Sequences

CONSTANTS Total, Quantities        \* Total : Nat ; Quantities : Seq(Nat), one per transfer

Transfers == 1..Len(Quantities)

VARIABLES
  uDonor, uReceiver, consumed,       \* usable rights and consumed units
  donorPhase, recvPhase,             \* per transfer: "Absent"/"Prepared"/"Committed"/"Aborted" and
                                     \*               "Absent"/"Accepted"/"Applied"/"Aborted"
  prepareSent, acceptSent, commitSent, abortSent,   \* messages once sent stay deliverable
  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals

vars == << uDonor, uReceiver, consumed, donorPhase, recvPhase,
           prepareSent, acceptSent, commitSent, abortSent,
           donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

Init ==
  /\ uDonor = Total /\ uReceiver = 0 /\ consumed = 0
  /\ donorPhase = [t \in Transfers |-> "Absent"]
  /\ recvPhase  = [t \in Transfers |-> "Absent"]
  /\ prepareSent = [t \in Transfers |-> FALSE] /\ acceptSent = [t \in Transfers |-> FALSE]
  /\ commitSent  = [t \in Transfers |-> FALSE] /\ abortSent  = [t \in Transfers |-> FALSE]
  /\ donorClosed = FALSE /\ replicaView = 0 /\ replicaSpends = 0 /\ spendsAfterClose = 0
  /\ revivals = 0

Q(t) == Quantities[t]

DonorPrepare(t) ==
  /\ donorPhase[t] = "Absent" /\ ~donorClosed /\ uDonor >= Q(t)
  /\ uDonor' = uDonor - Q(t) /\ donorPhase' = [donorPhase EXCEPT ![t] = "Prepared"]
  /\ UNCHANGED << uReceiver, consumed, recvPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

SendPrepare(t) ==
  /\ donorPhase[t] = "Prepared" /\ prepareSent' = [prepareSent EXCEPT ![t] = TRUE]
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, recvPhase, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

ReceiverAccept(t) ==
  /\ prepareSent[t] /\ recvPhase[t] = "Absent"
  /\ recvPhase' = [recvPhase EXCEPT ![t] = "Accepted"]
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

SendAccept(t) ==
  /\ recvPhase[t] = "Accepted" /\ acceptSent' = [acceptSent EXCEPT ![t] = TRUE]
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, recvPhase, prepareSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

DonorCommit(t) ==
  /\ acceptSent[t] /\ donorPhase[t] = "Prepared"
  /\ donorPhase' = [donorPhase EXCEPT ![t] = "Committed"]
  /\ UNCHANGED << uDonor, uReceiver, consumed, recvPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

SendCommit(t) ==
  /\ donorPhase[t] = "Committed" /\ commitSent' = [commitSent EXCEPT ![t] = TRUE]
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, recvPhase, prepareSent, acceptSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

DonorAbort(t) ==
  /\ donorPhase[t] = "Prepared"
  /\ donorPhase' = [donorPhase EXCEPT ![t] = "Aborted"] /\ uDonor' = uDonor + Q(t)
  /\ UNCHANGED << uReceiver, consumed, recvPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

SendAbort(t) ==
  /\ donorPhase[t] = "Aborted" /\ abortSent' = [abortSent EXCEPT ![t] = TRUE]
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, recvPhase, prepareSent, acceptSent, commitSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

ReceiverInstall(t) ==
  /\ commitSent[t] /\ recvPhase[t] = "Accepted"
  /\ recvPhase' = [recvPhase EXCEPT ![t] = "Applied"] /\ uReceiver' = uReceiver + Q(t)
  /\ UNCHANGED << uDonor, consumed, donorPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

ReceiverAbort(t) ==
  /\ abortSent[t] /\ recvPhase[t] \in {"Absent", "Accepted"}
  /\ recvPhase' = [recvPhase EXCEPT ![t] = "Aborted"]
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

SpendDonor ==
  /\ uDonor > 0 /\ ~donorClosed
  /\ uDonor' = uDonor - 1 /\ consumed' = consumed + 1
  /\ UNCHANGED << uReceiver, donorPhase, recvPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

SpendReceiver ==
  /\ uReceiver > 0
  /\ uReceiver' = uReceiver - 1 /\ consumed' = consumed + 1
  /\ UNCHANGED << uDonor, donorPhase, recvPhase, prepareSent, acceptSent, commitSent, abortSent,
                  donorClosed, replicaView, replicaSpends, spendsAfterClose, revivals >>

CloseDonorAuthority ==
  /\ ~donorClosed /\ donorClosed' = TRUE /\ replicaView' = uDonor
  /\ UNCHANGED << uDonor, uReceiver, consumed, donorPhase, recvPhase, prepareSent, acceptSent, commitSent,
                  abortSent, replicaSpends, spendsAfterClose, revivals >>

Next ==
  \/ \E t \in Transfers :
       DonorPrepare(t) \/ SendPrepare(t) \/ ReceiverAccept(t) \/ SendAccept(t) \/ DonorCommit(t)
       \/ SendCommit(t) \/ DonorAbort(t) \/ SendAbort(t) \/ ReceiverInstall(t) \/ ReceiverAbort(t)
  \/ SpendDonor \/ SpendReceiver \/ CloseDonorAuthority

Spec == Init /\ [][Next]_vars

InTransit(t) == IF donorPhase[t] \in {"Prepared", "Committed"} /\ recvPhase[t] # "Applied" THEN Q(t) ELSE 0
X == LET f[i \in 0..Len(Quantities)] == IF i = 0 THEN 0 ELSE f[i-1] + InTransit(i) IN f[Len(Quantities)]

Conservation == Total = consumed + uDonor + uReceiver + X
CreditImpliesCommit == \A t \in Transfers : recvPhase[t] = "Applied" => donorPhase[t] = "Committed"
NoRefundAfterCommit == \A t \in Transfers : donorPhase[t] = "Aborted" => recvPhase[t] # "Applied"
NoPassiveSpend == replicaSpends = 0
NoSpendAfterClose == spendsAfterClose = 0
NoLatePrepareResurrection == revivals = 0

Safety == Conservation /\ CreditImpliesCommit /\ NoRefundAfterCommit /\ NoPassiveSpend
          /\ NoSpendAfterClose /\ NoLatePrepareResurrection

(* Negative controls (each must violate Safety when enabled): refund after COMMITTED,   *)
(* duplicate install credit, stale replica spend, spend after close, late PREPARE       *)
(* revival. See fm1.rs `Control`.                                                       *)
=============================================================================
