/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import Cookies from 'js-cookie';

export abstract class CookieHelper {
  static getCookie(): string | undefined {
    const token = Cookies.get('cbs_token');

    if (!token) return undefined;

    if (token.startsWith("b'") && token.endsWith("'")) {
      return token.slice(2, -1);
    }

    return token;
  }

  static removeCookie() {
    Cookies.remove('cbs_token');
  }
}
