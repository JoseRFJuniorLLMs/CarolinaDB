-------------------------- MODULE FM3_Migration --------------------------
(* FM-3 — Migration, fencing and plan evolution (SPEC-009 §7–§8, SPEC-011 §4/§7).       *)
(* Same machine as crates/carolina-models/src/fm3.rs; the Rust checker is the executed  *)
(* artifact and a TLC run must be recorded separately.                                   *)

EXTENDS Naturals

CONSTANTS S, MaxPending, MaxAdmissions, MaxRevision
Sources == 1..S
Workers == 1..2

VARIABLES
  phase,             \* "Proposed" | "Closing" | "Draining" | "Installing" | "Active" | "Retired" | "Cancelled"
  catalogRevision, claim,            \* claim[w] = revision held by worker w (0 = none)
  admitting, fenced, offline, pending, closeCert,   \* per source
  targetInstalled, targetAdmitting,
  produced, retained, lateAdmissions, reopened

vars == << phase, catalogRevision, claim, admitting, fenced, offline, pending, closeCert,
           targetInstalled, targetAdmitting, produced, retained, lateAdmissions, reopened >>

Init ==
  /\ phase = "Proposed" /\ catalogRevision = 0 /\ claim = [w \in Workers |-> 0]
  /\ admitting = [s \in Sources |-> TRUE] /\ fenced = [s \in Sources |-> FALSE]
  /\ offline = [s \in Sources |-> FALSE] /\ pending = [s \in Sources |-> 0]
  /\ closeCert = [s \in Sources |-> FALSE]
  /\ targetInstalled = FALSE /\ targetAdmitting = FALSE
  /\ produced = 0 /\ retained = 0 /\ lateAdmissions = 0 /\ reopened = 0

Current(w) == claim[w] = catalogRevision /\ claim[w] > 0
Admitted == produced + pending[1] + (IF S >= 2 THEN pending[2] ELSE 0)

WorkerClaim(w) ==
  /\ ~Current(w) /\ catalogRevision < MaxRevision
  /\ catalogRevision' = catalogRevision + 1 /\ claim' = [claim EXCEPT ![w] = catalogRevision + 1]
  /\ UNCHANGED << phase, admitting, fenced, offline, pending, closeCert, targetInstalled, targetAdmitting,
                  produced, retained, lateAdmissions, reopened >>

StartClosing(w) == /\ Current(w) /\ phase = "Proposed" /\ phase' = "Closing"
                   /\ UNCHANGED << catalogRevision, claim, admitting, fenced, offline, pending, closeCert,
                                   targetInstalled, targetAdmitting, produced, retained, lateAdmissions, reopened >>
Cancel(w) == /\ Current(w) /\ phase = "Proposed" /\ phase' = "Cancelled"
             /\ UNCHANGED << catalogRevision, claim, admitting, fenced, offline, pending, closeCert,
                             targetInstalled, targetAdmitting, produced, retained, lateAdmissions, reopened >>
StartDraining(w) == /\ Current(w) /\ phase = "Closing" /\ \A s \in Sources : closeCert[s] /\ phase' = "Draining"
                    /\ UNCHANGED << catalogRevision, claim, admitting, fenced, offline, pending, closeCert,
                                    targetInstalled, targetAdmitting, produced, retained, lateAdmissions, reopened >>
DrainComplete(w) == /\ Current(w) /\ phase = "Draining" /\ \A s \in Sources : pending[s] = 0 /\ phase' = "Installing"
                    /\ UNCHANGED << catalogRevision, claim, admitting, fenced, offline, pending, closeCert,
                                    targetInstalled, targetAdmitting, produced, retained, lateAdmissions, reopened >>
InstallTarget(w) == /\ Current(w) /\ phase = "Installing" /\ ~targetInstalled /\ targetInstalled' = TRUE
                    /\ UNCHANGED << phase, catalogRevision, claim, admitting, fenced, offline, pending, closeCert,
                                    targetAdmitting, produced, retained, lateAdmissions, reopened >>
Activate(w) ==
  /\ Current(w) /\ phase = "Installing" /\ targetInstalled
  /\ \A s \in Sources : closeCert[s] /\ pending[s] = 0
  /\ phase' = "Active" /\ targetAdmitting' = TRUE
  /\ UNCHANGED << catalogRevision, claim, admitting, fenced, offline, pending, closeCert, targetInstalled,
                  produced, retained, lateAdmissions, reopened >>
Supersede(w) ==
  /\ Current(w) /\ phase \in {"Closing", "Draining", "Installing"}
  /\ phase' = "Cancelled" /\ targetAdmitting' = FALSE
  /\ fenced' = [s \in Sources |-> FALSE] /\ admitting' = [s \in Sources |-> TRUE]
  /\ closeCert' = [s \in Sources |-> FALSE]
  /\ UNCHANGED << catalogRevision, claim, offline, pending, targetInstalled, produced, retained,
                  lateAdmissions, reopened >>
Retire(w) == /\ Current(w) /\ phase = "Active" /\ phase' = "Retired"
             /\ UNCHANGED << catalogRevision, claim, admitting, fenced, offline, pending, closeCert, targetInstalled,
                             targetAdmitting, produced, retained, lateAdmissions, reopened >>

AdmitOld(s) ==
  /\ admitting[s] /\ ~fenced[s] /\ pending[s] < MaxPending /\ Admitted < MaxAdmissions
  /\ pending' = [pending EXCEPT ![s] = pending[s] + 1]
  /\ UNCHANGED << phase, catalogRevision, claim, admitting, fenced, offline, closeCert, targetInstalled,
                  targetAdmitting, produced, retained, lateAdmissions, reopened >>
ResolveOld(s) ==
  /\ pending[s] > 0 /\ ~offline[s]
  /\ pending' = [pending EXCEPT ![s] = pending[s] - 1] /\ produced' = produced + 1 /\ retained' = retained + 1
  /\ UNCHANGED << phase, catalogRevision, claim, admitting, fenced, offline, closeCert, targetInstalled,
                  targetAdmitting, lateAdmissions, reopened >>
FenceSource(s) ==
  /\ phase = "Closing" /\ ~fenced[s] /\ ~offline[s]
  /\ fenced' = [fenced EXCEPT ![s] = TRUE] /\ admitting' = [admitting EXCEPT ![s] = FALSE]
  /\ UNCHANGED << phase, catalogRevision, claim, offline, pending, closeCert, targetInstalled, targetAdmitting,
                  produced, retained, lateAdmissions, reopened >>
CloseCertificate(s) ==
  /\ fenced[s] /\ ~closeCert[s] /\ ~offline[s] /\ closeCert' = [closeCert EXCEPT ![s] = TRUE]
  /\ UNCHANGED << phase, catalogRevision, claim, admitting, fenced, offline, pending, targetInstalled,
                  targetAdmitting, produced, retained, lateAdmissions, reopened >>
ToggleOffline(s) ==
  /\ offline' = [offline EXCEPT ![s] = ~offline[s]]
  /\ UNCHANGED << phase, catalogRevision, claim, admitting, fenced, pending, closeCert, targetInstalled,
                  targetAdmitting, produced, retained, lateAdmissions, reopened >>

Next ==
  \/ \E w \in Workers : WorkerClaim(w) \/ StartClosing(w) \/ Cancel(w) \/ StartDraining(w) \/ DrainComplete(w)
                        \/ InstallTarget(w) \/ Activate(w) \/ Retire(w) \/ Supersede(w)
  \/ \E s \in Sources : AdmitOld(s) \/ ResolveOld(s) \/ FenceSource(s) \/ CloseCertificate(s) \/ ToggleOffline(s)

Spec == Init /\ [][Next]_vars

NoOverlap == ~(targetAdmitting /\ \E s \in Sources : admitting[s])
NoReopen == reopened = 0
ResultsResolvable == retained = produced
ActiveNeedsEvidence == phase \in {"Active", "Retired"} =>
   targetInstalled /\ \A s \in Sources : closeCert[s] /\ pending[s] = 0
NoLateAdmission == lateAdmissions = 0
FencedNeverAdmits == \A s \in Sources : fenced[s] => ~admitting[s]

Safety == NoOverlap /\ NoReopen /\ ResultsResolvable /\ ActiveNeedsEvidence /\ NoLateAdmission /\ FencedNeverAdmits
=============================================================================
