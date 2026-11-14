// Writes one line per function: entry, body size, address range, name,
// and where the name came from (DEFAULT = Ghidra's FUN_xxx, IMPORTED =
// the XBE loader / XbSymbolDatabase, USER_DEFINED = renamed by hand).
// @category h2recomp
import java.io.PrintWriter;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;

public class ExportFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        String out = args.length > 0 ? args[0] : "functions.tsv";
        int n = 0;
        try (PrintWriter w = new PrintWriter(out)) {
            w.println("entry\tsize\tmin\tmax\tsource\tname");
            for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
                var body = f.getBody();
                w.printf("%s\t%d\t%s\t%s\t%s\t%s%n",
                    f.getEntryPoint(), body.getNumAddresses(),
                    body.getMinAddress(), body.getMaxAddress(),
                    f.getSymbol().getSource(), f.getName(true));
                n++;
            }
        }
        println("exported " + n + " functions to " + out);
    }
}
