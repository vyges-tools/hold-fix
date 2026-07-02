module top ( clk, d, q ); input clk, d; output q; wire q1;
  DFF f1 ( .CK(clk), .D(d),  .Q(q1) );
  DFF f2 ( .CK(clk), .D(q1), .Q(q)  );
endmodule
