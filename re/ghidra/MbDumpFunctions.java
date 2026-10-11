// Decompiles functions from the headless rekordbox project into ~/re-work/decomp (outside the repo).
// Args: <outDir> <query>...   query = 0xADDRESS | exact qualified name | substring (prefix with ~)
// @category musicbox
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import java.io.File;
import java.io.PrintWriter;
import java.util.ArrayList;
import java.util.List;

public class MbDumpFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);
        for (int i = 1; i < args.length; i++) {
            String q = args[i];
            List<Function> found = new ArrayList<>();
            if (q.startsWith("0x")) {
                Function f = getFunctionContaining(toAddr(q));
                if (f != null) found.add(f);
            } else {
                boolean sub = q.startsWith("~");
                String needle = sub ? q.substring(1) : q;
                for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
                    String name = f.getName(true);
                    if (sub ? name.contains(needle) : name.equals(needle)) found.add(f);
                    if (found.size() >= 200) break;
                }
            }
            if (found.isEmpty()) println("NOT FOUND: " + q);
            for (Function f : found) {
                DecompileResults r = decomp.decompileFunction(f, 180, monitor);
                String body = r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// decompile failed: " + r.getErrorMessage() + "\n";
                String safe = f.getName(true).replaceAll("[^A-Za-z0-9_]+", "_");
                if (safe.length() > 120) safe = safe.substring(0, 120);
                File out = new File(outDir, f.getEntryPoint().toString() + "_" + safe + ".c");
                try (PrintWriter w = new PrintWriter(out, "UTF-8")) {
                    w.println("// " + f.getName(true) + " @ 0x" + f.getEntryPoint());
                    w.println("// signature: " + f.getSignature().getPrototypeString());
                    w.print(body);
                }
                println("WROTE " + out.getPath());
            }
        }
        decomp.dispose();
    }
}
