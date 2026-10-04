/**
 * Case-insensitive substring search over product names, in catalog order.
 * @param {{sku: string, name: string}[]} products
 * @param {string} term
 */
export function searchProducts(products, term) {
  const needle = term.trim().toLowerCase();
  return products.filter((product) => product.name.toLowerCase().includes(needle));
}
