import Carolina.Escrow

/-!
Axiom audit. A proof that depends on `sorryAx` is not a proof, so this module prints the axiom set
of every top-level theorem and `tools/run_lean.py` fails the build if `sorryAx` appears.
-/

#print axioms Carolina.Escrow.safety
#print axioms Carolina.Escrow.step_conserved
#print axioms Carolina.Escrow.step_good
#print axioms Carolina.Escrow.credit_implies_commit
#print axioms Carolina.Escrow.no_refund_after_commit
